# sensilo v2

[![Build status][workflow-badge]][workflow]

A generic WiFi sensor node based on the ESP-C3. Firmware written in Rust with
[esp-idf-hal](https://github.com/esp-rs/esp-idf-hal).


## History

This is a continuation of the previous [sensilo
project](https://github.com/dbrgn/sensilo), which used an nRF52832 based BLE
board. The idea did work, but the module did not have sufficient transmit power
to cover multiple rooms with a single BLE gateway.

With version 2, I'm trying to rely on WiFi instead. This will probably rule out
the low-power variant with the battery, but on the other hand it simplifies the
system a lot.


## Project Name

"Sensilo" is the [Esperanto word for
"sensor"](https://en.bab.la/dictionary/esperanto-english/sensilo).


## Components

- [Firmware](./firmware/)


## License

This project is open source, both software and hardware. Look at the component
directories for more details.


<!-- Badges -->
[workflow]: https://github.com/dbrgn/sensilo-v2/actions?query=workflow%3ACI
[workflow-badge]: https://img.shields.io/github/actions/workflow/status/dbrgn/sensilo-v2/ci.yml?branch=main
