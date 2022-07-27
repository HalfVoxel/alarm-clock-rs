compile:
	docker build -t raspberry-alarm:1 ./docker
	cross build --release --features audio --target armv7-unknown-linux-gnueabihf
copy: compile
	ssh pi@192.168.1.129 "sudo systemctl stop alarm"
	scp target/armv7-unknown-linux-gnueabihf/release/alarm pi@192.168.1.129:/home/pi/alarm
	ssh pi@192.168.1.129 "sudo systemctl start alarm"
