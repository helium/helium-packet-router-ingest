# Helium Packet Router Protocols
Backend interfaces meant to be run downstream from the [Helium Packet Router](https://github.com/helium/helium-packet-router/).

Create a Route in the [Config Service](https://github.com/helium/oracles/tree/main/iot_config) using the [Helium Config Service CLI](https://github.com/helium/helium-config-service-cli).
Set the protocol of the route to `packet_router`, and point it to wherever this service is running.

All available settings are in [settings/default.toml](settings/default.toml).
Override the settings you need to change, and that will be applied on top of `settings/defualt.toml`.

### Running

There is a sample docker-compose.yml.
Or run directly. 

``` sh
cargo run settings.toml
```

#### Protocol

The protocol is determined by settings `protocol` field in the `settings.toml` to `"gwmp" | "http"`.
