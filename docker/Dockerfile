FROM rustembedded/cross:armv7-unknown-linux-gnueabihf-0.2.1

RUN echo "HIII"
RUN dpkg --add-architecture armhf
RUN apt-get update
RUN apt install --assume-yes libasound2-dev:armhf
# RUN apt install --assume-yes openssl:armhf
# RUN apt install --assume-yes libasound2-dev
#     apt-get install --assume-yes libfoo:arm64

# RUN pkg-config --libs alsa
# RUN ls /usr/lib/x86_64-linux-gnu/libasound.so

ENV PKG_CONFIG_PATH=/usr/lib/arm-linux-gnueabihf/pkgconfig PKG_CONFIG_ALLOW_CROSS=1