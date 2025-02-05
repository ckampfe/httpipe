# httpipe

[![Rust](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml/badge.svg)](https://github.com/ckampfe/httpipe/actions/workflows/rust.yml)


`httpipe` is an HTTP server that allows for HTTP clients to forward data to each other in a queue-like way using named channels.


## What is this 

In `httpipe`, channels are named, logical queues for conveying data from one HTTP request to another, so `httpipe` is like its name describes: a way to pipe data from one HTTP client to another, via a central server.

A producer enqueues data to send by sending a `POST` request to a channel.
A consumer receives that data by sending a `GET` request to the same channel.

For example:

```sh
# in one terminal, a producer client enqueues data...
curl -XPOST /channels/v1/do_work -d"hello"
```

```sh
# ...and in another terminal, a consumer client receives it
curl -XGET /channels/v1/do_work
hello%
```

The data was piped from the first request (the producer) to the second (the consumer).


## installation

```
$ cargo install --git https://github.com/ckampfe/httpipe.git
```

This downloads and builds `httpipe` from source from this repo, assuming you have [Rust installed](https://www.rust-lang.org/tools/install).


## Details

### Naming

Channels are uniquely identified by their namespace and name, both of which are required.

This is the URL scheme: `/channels/{namespace}/{channel_name}`.

For example, the channel `/channels/v1/do_work` has the namespace `v1` and the name `do_work`.

- There can be arbitrarily many namespaces.
- Namespaces are globally unique (the namespace `foo` always refers to the same namespace).
- Namespaces can have many arbitrarily many channels.
- Channel names are unique per namespace, meaning `/channels/v1/foo` is a different channel than `/channels/v2/foo`, even though both have the same channel name.

Both namespaces and channel names are automatically created on first use and cannot be manually created out of band.

Namespaces and channels can be destroyed manually by deleting either a specific channel (`DELETE /channels/{namespace}/{channel_name}`), or by deleting the namespace and all of its associated channels (`DELETE /channels/{namespace}`).


### Concurrency and order

The concurrency of a given channel is 1.

Channels are "rendezvous" queues, in that both producer and consumer requests to a given channel will block until there is a counterpart on the other side of the channel to either receive or produce data, respectively. That is, a producer will block until there is a consumer, and a consumer will block until there is a producer.

There can be arbitrarily many producer and consumer requests currently pending against an instance of `httpipe`, but only one producer-consumer pair at a time for a given channel can be exchanging data.

Producers and consumers are matched in the order in which they made their requests.
You can think of this logically as there being a "producer queue" and a "consumer queue".

For example, if we make 4 consecutive producer requests to `/channels/v1/do_work` (call them `p1`, `p2`, `p3`, and `p4`), they would all block until a consumer connects for each of them, because we have enqueued 4 producer requests into the "producer queue".

If we then make a single consumer request to `/channels/v1/do_work` (call it `c1`), `httpipe` will match `p1` with `c1` by popping `p1` off the "producer queue" and popping `c1` off the "consumer queue", sending `p1`'s data to `c1`. Producer requests `p2`, `p3`, and `p4` would still remain blocking in the "producer queue", waiting for consumers to connect.

(Note that the "producer queue" and "consumer queue" terms are just metaphors. `httpipe` implements this behavior in a slightly different way, but this "queue" logic still applies.)


### Use cases

It turns out this functionality, limited as it is, is enough to do a lot of useful stuff.
You can use `httpipe` to send notifications, build a concurrent job queue, share files, chat, and all kinds of other stuff. See the [archived patchbay site](https://web.archive.org/web/20241105063704/https://patchbay.pub/) for other ideas.


### Options

```
Usage: httpipe [OPTIONS]

Options:
  -p, --port <PORT>
          the port to bind the server to [env: PORT=] [default: 3000]
  -r, --request-timeout <REQUEST_TIMEOUT>
          the maximum request timeout, in seconds [env: REQUEST_TIMEOUT=]
  -h, --help
          Print help
```


### Credit

This is not my idea! I first discovered this idea a few years ago as [patchbay](https://web.archive.org/web/20241105063704/https://patchbay.pub/) by [Anders Pitman](https://github.com/anderspitman). Recently I remembered patchbay, and found out that it is apparently no longer available, so I decided to try to build my own based on what I could read on the archive of the patchbay site.

Note that this project is an entirely new work and does not draw from or use the original patchbay code in any way, so any flaws in it are mine alone.

### Todo

This originally had patchbay's additional pubsub functionality but I removed it while I work on the design. I may add it back at some point, or not.
