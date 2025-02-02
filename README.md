# httpipe

[![Rust](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml/badge.svg)](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml)

This allows forwarding data from client to client over HTTP via a central server.

This is not my idea!

This is a reimplementation of [patchbay](https://web.archive.org/web/20241105063704/https://patchbay.pub/). I love the idea of patchbay, and patchbay seems to be no longer available, and I wanted an implementation.

There are two ways to use this so far, copied directly from patchbay: channels and pubsub. With channels, producers and consumers will block until the other side connects, and with pubsub, producers will not block, and will send immediately to any available consumers, even if none are available.

## Examples

### Channels (blocking)

In terminal #1

```sh
cargo run
```

In terminal #2

```sh
$ curl -XPOST http://localhost:3000/channels/create   
bEE4RvrHRbOTTkMaPOgY% 

$ curl -XPOST -H "Content-Type: application/json" http://localhost:3000/channels/bEE4RvrHRbOTTkMaPOgY -d'{"greeting":"bwah!"}' 
```

In terminal #3

```sh
$ curl -N -XGET http://localhost:3000/channels/bEE4RvrHRbOTTkMaPOgY   
{"greeting":"bwah!"}%    
```

### Pubsub (non-blocking)

Todo
