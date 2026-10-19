use crate::{
    MAX_BODY, OpenAiCompatible, contract, decode_completion, encode_messages,
    services::{Format, Service, wire_error},
};
use namzu_core::{
    AgentError, Completion, Event, EventSink, Message, Provider, ToolDefinition, TurnContext,
};
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};
use std::io::{BufReader, Read};
impl OpenAiCompatible {
    pub(crate) fn replay_route(&self) -> String {
        json!({"service":self.service,"endpoint":self.endpoint.as_str(),"model":self.model,"options":self.options}).to_string()
    }
    fn chat_messages(&self, messages: &[Message], route: &str) -> Result<Vec<Value>, AgentError> {
        let ordered = namzu_core::ordered_messages(messages);
        let mut encoded = encode_messages(messages);
        let mut position = 0;
        let mut state: Option<&Value> = None;
        for message in ordered {
            if let Message::ProviderState { data } = message {
                state = Some(data);
                continue;
            }
            if let Message::Assistant {
                content,
                tool_calls,
            } = message
            {
                if let Some(saved) = state.filter(|s| {
                    s["kind"] == "namzu.api.chat"
                        && s["route"] == route
                        && s["signature"] == json!({"text":content,"tool_calls":tool_calls})
                }) {
                    if saved["credential_tag"] != self.credential_tag {
                        return Err(AgentError::Configuration(
                            "compatible replay credential changed; start a new session",
                        ));
                    }
                    for key in ["reasoning_content", "reasoning_details", "reasoning"] {
                        if let Some(v) = saved.get(key) {
                            encoded[position][key] = v.clone();
                        }
                    }
                }
                if self.service == Service::ZenGo
                    && self.model.starts_with("deepseek-")
                    && encoded[position].get("reasoning_content").is_none()
                {
                    encoded[position]["reasoning_content"] = "".into();
                }
            }
            state = None;
            position += 1;
        }
        Ok(encoded)
    }
    pub(crate) fn perform(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        ctx: &TurnContext,
        events: &mut dyn EventSink,
    ) -> Result<Completion, AgentError> {
        self.metadata = None;
        self.replay = None;
        namzu_core::provider::validate_request(messages, tools, &self.info(), ctx)?;
        if self.format.native() {
            return self.perform_native(messages, tools, ctx, events);
        }
        let route = self.replay_route();
        let mut body = if self.format == Format::Responses {
            let mut safe = Vec::new();
            for message in messages {
                if let Message::ProviderState { data } = message
                    && data["kind"] == "namzu.api.responses"
                    && data["route"] == route
                    && data["credential_tag"] != self.credential_tag
                {
                    return Err(AgentError::Configuration(
                        "Responses replay credential changed",
                    ));
                }
                safe.push(message.clone());
            }
            json!({"model":self.model,"stream":self.options.stream,"store":false,"input":crate::codex::encode_input_for(&safe,&route,"namzu.api.responses")?,"tools":tools.iter().map(|t|json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters,"strict":false})).collect::<Vec<_>>()})
        } else {
            json!({"model":self.model,"stream":self.options.stream,"messages":self.chat_messages(messages,&route)?})
        };
        if self.format == Format::Responses {
            if self.model.starts_with("gpt-")
                || self.model.as_bytes().first() == Some(&b'o')
                    && self.model.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
            {
                body["include"] = json!(["reasoning.encrypted_content"]);
            }
            let instructions = messages
                .iter()
                .filter_map(|m| {
                    if let Message::System { content } = m {
                        Some(content.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !instructions.is_empty() {
                body["instructions"] = instructions.into();
            }
            if let Some(max) = self.options.max_output_tokens {
                body["max_output_tokens"] = max.into();
            }
        } else {
            if !tools.is_empty() {
                body["tools"]=tools.iter().map(|t|json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect::<Vec<_>>().into();
            }
            if self.options.stream {
                body["stream_options"] = json!({"include_usage":true});
            }
            if let Some(max) = self.options.max_output_tokens {
                let modern = self.service == Service::OpenaiCompatible
                    && (self.model.starts_with("gpt-5")
                        || self.model.starts_with("gpt-6")
                        || self.model.as_bytes().first() == Some(&b'o')
                            && self.model.as_bytes().get(1).is_some_and(u8::is_ascii_digit));
                body[if modern {
                    "max_completion_tokens"
                } else {
                    "max_tokens"
                }] = max.into();
            }
        }
        if let Some(thinking) = &self.options.thinking {
            body["thinking"] = json!({"type":thinking});
        }
        if self.service == Service::Openrouter {
            body["provider"] = json!({"allow_fallbacks":false,"require_parameters":true});
        }
        if let Some(effort) = &self.options.reasoning_effort {
            if self.format == Format::Responses || self.service == Service::Openrouter {
                body["reasoning"] = json!({"effort":effort});
            } else {
                body["reasoning_effort"] = effort.clone().into();
            }
        }
        if serde_json::to_vec(&body)
            .map_err(|_| AgentError::Protocol("cannot encode compatible request"))?
            .len()
            > 8 * 1024 * 1024
        {
            return Err(AgentError::Configuration(
                "compatible wire request exceeds limit",
            ));
        }
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .timeout(ctx.remaining()?)
            .json(&body);
        if let Some(auth) = &self.authorization {
            request = request.header(AUTHORIZATION, auth.clone());
        }
        if let Some(site) = &self.options.site_url {
            request = request.header("HTTP-Referer", site);
        }
        if let Some(title) = &self.options.site_name {
            request = request.header("X-Title", title);
        }
        let response = self.client.send(request, ctx)?;
        ctx.remaining()?;
        if !response.status().is_success() {
            return Err(contract::status(&response));
        }
        let (completion, mut metadata, replay) = if self.options.stream
            && self.format == Format::Responses
        {
            let (completion, items, metadata) =
                crate::codex::sse::parse_stream_metadata(BufReader::new(response), ctx, events)?;
            let state = json!({"kind":"namzu.api.responses","route":route,"credential_tag":self.credential_tag,"signature":{"text":completion.text,"tool_calls":completion.tool_calls},"items":items});
            (completion, metadata, Some(state))
        } else {
            let (value, reported_cost) = if self.options.stream {
                crate::chat_stream::parse(
                    BufReader::new(response),
                    ctx,
                    events,
                    self.service == Service::Openrouter,
                )?
            } else {
                let mut bytes = vec![];
                response
                    .take(MAX_BODY + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| crate::http::read_error(ctx))?;
                ctx.remaining()?;
                if bytes.len() as u64 > MAX_BODY {
                    return Err(AgentError::Protocol("HTTP response exceeds 4 MiB"));
                }
                let cost = if self.service == Service::Openrouter {
                    crate::services::raw_cost(&bytes)?
                } else {
                    None
                };
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| AgentError::Protocol("invalid compatible response JSON"))?;
                (value, cost)
            };
            if let Some(error) = wire_error(&value) {
                return Err(error);
            }
            if self.format == Format::Responses {
                if value["status"] != "completed" {
                    return Err(AgentError::Protocol("Responses response incomplete"));
                }
                let items = value["output"]
                    .as_array()
                    .filter(|v| v.len() <= 4096)
                    .ok_or(AgentError::Protocol("invalid Responses output"))?;
                let completion = crate::codex::sse::decode(items)?;
                let metadata = contract::metadata(&value, true)?;
                let state = json!({"kind":"namzu.api.responses","route":route,"credential_tag":self.credential_tag,"signature":{"text":completion.text,"tool_calls":completion.tool_calls},"items":items});
                (completion, metadata, Some(state))
            } else {
                let completion = decode_completion(&value)?;
                let mut metadata = contract::metadata(&value, false)?;
                if self.service == Service::Openrouter {
                    metadata.cost = reported_cost;
                }
                let message = &value["choices"][0]["message"];
                let mut state = json!({"kind":"namzu.api.chat","route":route,"credential_tag":self.credential_tag,"signature":{"text":completion.text,"tool_calls":completion.tool_calls}});
                let mut captured = false;
                for key in ["reasoning_content", "reasoning_details", "reasoning"] {
                    if let Some(v) = message.get(key).filter(|v| !v.is_null()) {
                        if ((key == "reasoning_content" || key == "reasoning") && !v.is_string())
                            || (key == "reasoning_details" && !v.is_array())
                        {
                            return Err(AgentError::Protocol("invalid compatible reasoning state"));
                        }
                        state[key] = v.clone();
                        captured = true;
                    }
                }
                (
                    completion,
                    metadata,
                    if captured { Some(state) } else { None },
                )
            }
        };
        namzu_core::provider::validate_completion(&completion)?;
        ctx.remaining()?;
        if metadata.cost.is_none()
            && let (Some(pricing), Some(usage)) = (&self.pricing, &metadata.usage)
        {
            metadata.cost = pricing.estimate(usage)?;
        }
        if let Some(state) = &replay
            && state.to_string().len() > 4 * 1024 * 1024
        {
            return Err(AgentError::Protocol(
                "compatible reasoning state exceeds limit",
            ));
        }
        if !self.options.stream && !completion.text.is_empty() {
            events.emit(Event::AssistantDelta {
                text: completion.text.clone(),
            })?;
            ctx.remaining()?;
        }
        self.metadata = Some(metadata);
        self.replay = replay;
        Ok(completion)
    }
}
