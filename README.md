# Mnemo

An ephemeral in-memory key-value store. Its goals are speed and memory efficiency. It solves very specific problems with very specific solutions. It is not intended to be a generic store, like Redis.

It does and probably will allways do only GET and UPDATE.

For now the only purpose it serves is for learning.

It tries its best to avoid memory contention. The main version is single threaded using Tokio's LocalRuntime for IO and concurrent CPU time.
I will be experimenting with various concurrency paradigms, including even using my future own async runtime (lol).

The main language used is Rust. I have also used Go and Python for the ease of writing some test clients.
