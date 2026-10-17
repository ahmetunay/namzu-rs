ARG BASE_IMAGE=debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
FROM ${BASE_IMAGE}
LABEL io.namzu.guest.protocol="1"
COPY --chmod=0555 namzu-guest /usr/local/bin/namzu-guest
RUN mkdir -p /workspace && chown 65532:65532 /workspace && chmod 0700 /workspace
USER 65532:65532
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/namzu-guest", "idle"]
