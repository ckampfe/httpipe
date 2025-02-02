# httpipe

[![Rust](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml/badge.svg)](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml)

This allows forwarding data from client to client over HTTP via a central server.

This is not my idea!

This is a reimplementation of [patchbay](https://web.archive.org/web/20241105063704/https://patchbay.pub/). I love the idea of patchbay, and patchbay seems to be no longer available, and I wanted an implementation.

There are two ways to use this so far, copied directly from patchbay: channels and pubsub. With channels, producers and consumers will block until the other side connects, and with pubsub, producers will not block, and will send immediately to any available consumers, even if none are available.

## Examples

### Channels (producers and consumers block until they rendezvous)

In terminal #1

```sh
cargo run
```

In terminal #2

This will block until there is a consumer.

```sh
$ curl -XPOST http://localhost:3000/channels
566f4825-d0b9-4712-b4e4-244884fa6b8c%

$ curl -XPOST -H "Content-Type: application/json" http://localhost:3000/channels/566f4825-d0b9-4712-b4e4-244884fa6b8c -d'{"greeting":"bwah!"}' 
```

In terminal #3

This will block until there is a producer.

```sh
$ curl -N -XGET http://localhost:3000/channels/566f4825-d0b9-4712-b4e4-244884fa6b8c
{"greeting":"bwah!"}%    
```

### Pubsub (consumers block until there is a producer, but producers _never_ block, and will publish even if there are no consumers)

In terminal #1

```sh
cargo run
```

In terminal #2

```sh
$ curl -XPOST http://localhost:3000/pubsub
566f4825-d0b9-4712-b4e4-244884fa6b8c%
```

In terminal #3

Note that this GET _must_ be made before any subsequent posts,
because for pubsub only GET is blocking. POST is nonblocking.

```sh
$ curl -N -XGET http://localhost:3000/pubsub/566f4825-d0b9-4712-b4e4-244884fa6b8c
{"greeting":"bwah!"}%    
```

In terminal #2

```sh
$ curl -XPOST -H "Content-Type: application/json" http://localhost:3000/pubsub/566f4825-d0b9-4712-b4e4-244884fa6b8c -d'{"greeting":"bwah!"}' 
```

## Options

```
Usage: httpipe [OPTIONS]

Options:
  -p, --port <PORT>                        [env: PORT=] [default: 3000]
  -r, --request-timeout <REQUEST_TIMEOUT>  [env: REQUEST_TIMEOUT=]
  -h, --help                               Print help
```

