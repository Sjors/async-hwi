FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential git python3 python3-pip python3-venv pkg-config \
    libffi-dev libsdl2-dev libsdl2-image-dev autoconf automake libtool \
    rsync ca-certificates curl xterm \
 && rm -rf /var/lib/apt/lists/*
RUN python3 -m pip install --no-cache-dir --break-system-packages \
    pysdl2 pillow namedlist pyusb
WORKDIR /work
