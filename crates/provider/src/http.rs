//! Synchronous SDK facade over cancellable async HTTP. No detached request thread.
use namzu_core::{AgentError, TurnContext};
use reqwest::{Request, RequestBuilder, StatusCode};
use std::{
    future::Future,
    io::{self, Read},
    sync::Arc,
    time::Duration,
};

pub(crate) struct Client {
    client: reqwest::Client,
    runtime: Arc<tokio::runtime::Runtime>,
}
pub(crate) struct Builder(reqwest::ClientBuilder);
impl Builder {
    pub fn redirect(mut self, policy: reqwest::redirect::Policy) -> Self {
        self.0 = self.0.redirect(policy);
        self
    }
    pub fn user_agent(mut self, value: &str) -> Self {
        self.0 = self.0.user_agent(value);
        self
    }
    #[cfg(test)]
    pub fn no_proxy(mut self) -> Self {
        self.0 = self.0.no_proxy();
        self
    }
    pub fn build(self) -> Result<Client, AgentError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| AgentError::Provider("cannot initialize HTTP runtime".into()))?;
        let client = self.0.build().map_err(error)?;
        Ok(Client {
            client,
            runtime: Arc::new(runtime),
        })
    }
}
fn error(error: reqwest::Error) -> AgentError {
    if error.is_timeout() {
        AgentError::Timeout
    } else {
        AgentError::Remote(namzu_core::ProviderFailure::transport())
    }
}
async fn wait<T>(
    context: &TurnContext,
    future: impl Future<Output = Result<T, reqwest::Error>>,
) -> Result<T, AgentError> {
    let remaining = context.remaining()?;
    tokio::select! {
        biased;
        error = async { loop {
            if let Err(error) = context.remaining() { return error; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        } } => Err(error),
        result = tokio::time::timeout(remaining, future) => {
            context.remaining()?;
            result.map_err(|_| AgentError::Timeout)?.map_err(error)
        }
    }
}
impl Client {
    pub fn builder() -> Builder {
        Builder(reqwest::Client::builder())
    }
    pub fn get(&self, url: reqwest::Url) -> RequestBuilder {
        self.client.get(url)
    }
    pub fn post(&self, url: reqwest::Url) -> RequestBuilder {
        self.client.post(url)
    }
    pub fn send(
        &self,
        request: RequestBuilder,
        context: &TurnContext,
    ) -> Result<Response, AgentError> {
        self.execute(request.build().map_err(error)?, context)
    }
    pub fn execute(&self, request: Request, context: &TurnContext) -> Result<Response, AgentError> {
        let response = self
            .runtime
            .block_on(async { wait(context, self.client.execute(request)).await })?;
        Ok(Response {
            response,
            runtime: self.runtime.clone(),
            context: context.clone(),
            pending: vec![],
            offset: 0,
        })
    }
}
pub(crate) struct Response {
    response: reqwest::Response,
    runtime: Arc<tokio::runtime::Runtime>,
    context: TurnContext,
    pending: Vec<u8>,
    offset: usize,
}
impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
    }
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }
}
impl Read for Response {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.context.remaining().map_err(io::Error::other)?;
        while self.offset == self.pending.len() {
            match self
                .runtime
                .block_on(wait(&self.context, self.response.chunk()))
                .map_err(io::Error::other)?
            {
                None => return Ok(0),
                Some(bytes) => {
                    self.pending = bytes.to_vec();
                    self.offset = 0;
                }
            }
        }
        let n = output.len().min(self.pending.len() - self.offset);
        output[..n].copy_from_slice(&self.pending[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}
pub(crate) fn read_error(context: &TurnContext) -> AgentError {
    context
        .remaining()
        .err()
        .unwrap_or(AgentError::Remote(namzu_core::ProviderFailure::transport()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use namzu_core::Cancellation;
    use std::{
        pin::Pin,
        sync::mpsc,
        task::{Context, Poll},
    };
    #[test]
    fn cancellation_drops_an_already_polled_network_future() {
        struct Pending {
            started: Option<mpsc::Sender<()>>,
            dropped: mpsc::Sender<()>,
        }
        impl Future for Pending {
            type Output = Result<(), reqwest::Error>;
            fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
                if let Some(tx) = self.started.take() {
                    tx.send(()).unwrap();
                }
                Poll::Pending
            }
        }
        impl Drop for Pending {
            fn drop(&mut self) {
                let _ = self.dropped.send(());
            }
        }
        let signal = Cancellation::default();
        let cancel = signal.clone();
        let (start, started) = mpsc::channel();
        let (drop_tx, dropped) = mpsc::channel();
        let cancel_thread = std::thread::spawn(move || {
            started.recv().unwrap();
            cancel.cancel();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let context = TurnContext::new(Duration::from_secs(30), signal).unwrap();
        assert_eq!(
            runtime.block_on(wait(
                &context,
                Pending {
                    started: Some(start),
                    dropped: drop_tx
                }
            )),
            Err(AgentError::Cancelled)
        );
        dropped.recv_timeout(Duration::from_secs(3)).unwrap();
        cancel_thread.join().unwrap();
    }
}
