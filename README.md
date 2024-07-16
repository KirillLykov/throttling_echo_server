## Throttling echo server on rust with quinn and tokio

Echo server with throttling implemented using [token bucket algorithm](https://en.wikipedia.org/wiki/Token_bucket).
Simplified adaptation of the [boost asio example](https://github.com/boostorg/asio/blob/develop/example/cpp20/channels/throttling_proxy.cpp).

The algorithm is the following:

1. define a bucket of current capacity `b` and of max capacity `B`
2. every `1/t` seconds 1 token added to the bucket
3. when packet of size `n` bytes arrives:

    3.1 if `b >= n` => `b := b - n` and process packet

    3.2 else packet is non-conformant

There are 2 different strategies to handle non-conformant packets: drop them or delay (so-called traffic shaping).

## Observations

Here, I'm trying to delay them.
But I don't observe that these delays are communicated to the sender.
I would expect that sending streams takes longer if processing them on the server side takes longer.
Yet according to the experiments in the `tests` folder, it is not the case.