# syntax=docker/dockerfile:1
ARG BASE_IMAGE=debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
FROM ${BASE_IMAGE}
RUN --mount=type=secret,id=system_ca,required=true \
    apt-get -o Acquire::https::CaInfo=/run/secrets/system_ca update && \
    apt-get -o Acquire::https::CaInfo=/run/secrets/system_ca install -y --no-install-recommends xvfb x11-xkb-utils chromium fonts-liberation && \
    rm -rf /var/lib/apt/lists/*
LABEL io.namzu.guest.protocol="1" io.namzu.desktop="x11-1" io.namzu.browser="chromium-cdp-1"
COPY --chmod=0555 namzu-guest namzu-desktop namzu-browser /usr/local/bin/
RUN mkdir -p /workspace && chown 65532:65532 /workspace && chmod 0700 /workspace
USER 65532:65532
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/namzu-guest", "idle"]
