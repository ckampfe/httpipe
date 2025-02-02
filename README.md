# httpipe

[![Rust](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml/badge.svg)](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml)

`httpipe` is an HTTP server that allows HTTP clients to publish and subscribe to each other.

Clients can interact via a queue-like mechanism ("channels") where a producer and a consumer are matched 1:1 in the order in which they connect, or a fanout mechanism ("pubsub") where a producer can publish a message to many consumers in parallel.

## Channels 

Think of channels as basically queues.

A producer (`POST /channels/{id}`) puts a message into the channel, waiting until there is a consumer to take it.

If there is no consumer, the producer call will block.

Likewise, a consumer (`GET /channels/{id}`) waits for a message, blocking if there isn't one.

Only one consumer will ever receive a given producer's message.

Producers and consumers are matched to each other in request order.

All actions take place within the context of a single HTTP request.
Producers publish their message and then disconnect.
Consumers receive a single message and then disconnect.

### Example

In terminal #1

```sh
# run the server
cargo run
```

In terminal #2

```sh
# create a channel, receiving its unique ID (a UUID)...
$ curl -XPOST http://localhost:3000/channels
566f4825-d0b9-4712-b4e4-244884fa6b8c%

# ...and then publish a message on that channel,
# blocking if there is not yet a consumer on the other side ready to receive it
$ curl -XPOST \
    -H "Content-Type: application/json" \
    http://localhost:3000/channels/566f4825-d0b9-4712-b4e4-244884fa6b8c \
    -d'{"greeting":"bwah!"}' 
```

In terminal #3

```sh
# receive a message from a channel, if there is one.
# this call will block until there is a message ready to consume.
# note that we disable curl's buffering with `-N` to make sure we receive the full response immediately.
$ curl -N -XGET http://localhost:3000/channels/566f4825-d0b9-4712-b4e4-244884fa6b8c
{"greeting":"bwah!"}%    
```

## Pubsub

Pubsub is a traditional fanout-style/broadcast-style 1:N ephemeral messaging mechanism.
Many consumers can listen on a given pubsub, and when a producer publishes on that pubsub,
every consumer currenlty listening to that pubsub will receive the message that producer sent.

Publishing to a pubsub (`POST /pubsub/{id}`) is non-blocking. A producer's POST to a pubsub always succeeds immediately,
even if there are no consumers listening.

Consuming from a pubsub (`GET /pubsub/{id}`) is blocking. Consumers will wait until they receive
a message. Any number of consumers can wait concurrently, but they will all receive the first
message to be published on a pubsub.

Like with channels, all actions take place within the context of a single HTTP request.
Producers publish their message and then disconnect.
Consumers receive a single message and then disconnect.

### Example

In terminal #1

```sh
# run the server
cargo run
```

In terminal #2

```sh
# create a pubsub and get its ID.
$ curl -XPOST http://localhost:3000/pubsub
566f4825-d0b9-4712-b4e4-244884fa6b8c%
```

In terminal #3

```sh
# have a consumer listen on this pubsub.
# note a few things:
# - we must listen first, because with pubsub, only consumers are blocking
# - you can repeat this command an arbitrary number of times, so an arbitrary number of clients can listen on this pubsub simultaneously, and all of them will receive the first message published after they connect
$ curl -N -XGET http://localhost:3000/pubsub/566f4825-d0b9-4712-b4e4-244884fa6b8c
{"greeting":"bwah!"}%    
```

In terminal #2

```sh
# publish a message on this pubsub, regardless of whether any consumers are listening
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

## Credit

This is not my idea! I first discovered this idea a few years ago as [patchbay](https://web.archive.org/web/20241105063704/https://patchbay.pub/) by [Anders Pitman](https://github.com/anderspitman). Recently I remembered patchbay, and found out that it is apparently no longer available, so I decided to try to build my own based on what I could read on the archive of the patchbay site.

Note that this project is an entirely new work and does not draw from or use the original patchbay code in any way, so any flaws in it are mine alone.
