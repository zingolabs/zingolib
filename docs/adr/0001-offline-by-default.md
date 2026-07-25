# LightClient starts offline by default

`ClientConfig::builder().build()` produces an offline client: it configures no Indexer until the consumer calls `set_indexer_uri()`. We chose this over a baked-in default of `zec.rocks:443` because the consumer should control when the client connects and to which server. The choice also prepares for multi-indexer support, in which a consumer configures several servers with different roles rather than relying on a single default.

## Considered Options

A default of `zec.rocks:443` would spare the casual consumer a configuration step, but it couples the library to a specific third-party server and performs network activity the consumer did not request.
