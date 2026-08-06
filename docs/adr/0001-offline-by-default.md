# LightClient starts offline by default

`ClientConfig::builder().build()` produces an offline client: no Indexer is configured until the consumer explicitly calls `set_indexer_uri()`. We chose this over defaulting to `zec.rocks:443` because the consumer should be in complete control of when and which server is connected to. This also lays the groundwork for future multi-indexer support, where a consumer may want to configure several servers with different roles rather than having one baked-in default.

## Considered Options

Defaulting to `zec.rocks:443` would be more ergonomic for simple use cases, but it couples the library to a specific third-party server and silently performs network activity the consumer did not request.
