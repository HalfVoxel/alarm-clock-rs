compile:
	docker build -t raspberry-alarm:1 ./docker
	cross build --release --features audio --target armv7-unknown-linux-gnueabihf