# syntax=docker/dockerfile:1
#
# BoxLite REST server (`boxlite serve`) for the sandbox stack.
#
# BoxLite starts a hardware-isolated microVM per box, so this container needs
# the host's KVM device. Run it privileged with /dev/kvm passed through (see
# deploy/compose.yaml, `boxlite` service).
#
# The `boxlite` binary embeds the runtime (boxlite-shim, boxlite-guest,
# libkrunfw), so only the CLI needs to be installed. OCI images are pulled on
# demand and cached under $HOME/.boxlite.

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        tar \
        iproute2 \
    && rm -rf /var/lib/apt/lists/*

# Install the CLI to /usr/local/bin (no PATH tweaking needed).
RUN curl -fsSL https://sh.boxlite.ai | BOXLITE_INSTALL_DIR=/usr/local/bin sh

ENV RUST_LOG=info

EXPOSE 8100

ENTRYPOINT ["boxlite"]
CMD ["serve"]
