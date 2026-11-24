# Interview Prep: How This Project Works

A deep-dive on the design of this Redis-like in-memory store, written to be read
before interviews. Everything below is grounded in the actual code in this repo
(~545 lines of Rust). Where the code and the README disagree, the code is right
and the difference is called out, because an interviewer with the repo open will
see it too.

---

## 1. What it is

A Redis-inspired, in-memory key-value store written in Rust on the tokio async
runtime. It listens on TCP (`127.0.0.1:7635`, hardcoded in `src/main.rs`) and
serves multiple clients concurrently. It implements four of Redis's core data
types: strings (with optional TTL), lists, sets, and sorted sets.

It is a learning project, not a production system, and that is a fine thing to
say out loud. The honest framing: "I built this to understand how Redis works
under the hood: the wire protocol, the data structures, concurrency, and
expiry."

## 2. Architecture at a glance

```
src/main.rs        entry point: starts the tokio runtime, binds the port
src/server/        TCP accept loop, one async task per connection
src/command/       command parsing and dispatch (match on the parsed command)
src/database/      the storage layer:
  db.rs            Database struct: five shared maps, expiry logic
  data_structure.rs  RList (VecDeque), RSets (HashSet), RSortedSet (HashMap + BTreeSet)
```

Request flow, end to end:

1. `main` calls `create_server("127.0.0.1:7635")`.
2. `create_server` binds a `TcpListener` and loops on `accept()`.
3. For each accepted socket it `tokio::spawn`s a new async task, passing a cheap
   clone of the `Database` (the clone only clones `Arc` pointers, not data).
4. Inside the task: wrap the socket's read half in a `BufReader`, loop on
   `read_line`, pass each line to `command_parser`, and write the response back.
5. `command_parser` splits the line on whitespace and pattern-matches on the
   resulting slice (`["SET", key, value]`, `["GET", key]`, ...), then calls the
   matching `Database` method.
6. The `Database` takes a lock on the relevant map, performs the operation, and
   the command layer formats the reply.

If asked to draw it: one listener task, one task per client, all tasks sharing
one `Database` behind `Arc`.

## 3. The protocol (read this carefully, it is the most honest part)

**Responses are RESP-encoded. Requests are not.** This is the single most
likely "gotcha" question.

- The server reads requests with `BufReader::read_line` and splits on
  whitespace. That is Redis's *inline command* style: `SET name ahmed` as one
  plain-text line. It does **not** parse RESP array requests
  (`*3\r\n$3\r\nSET\r\n...`), which is what real clients like `redis-cli` send.
- Replies, on the other hand, are formatted as RESP: `+OK\r\n` (simple string),
  `$-1\r\n` (null bulk string), `$5\r\nahmed\r\n` (bulk string), `*3\r\n...`
  (array), `:1\r\n` (integer).

Consequences you should be ready to explain:

- You test it with `nc` / `telnet`, not `redis-cli`, because `redis-cli` speaks
  RESP on the request side (see section 7).
- Real Redis accepts both inline commands and RESP; this project accepts inline
  only. Making it client-compatible means writing a RESP request parser:
  read `*<n>`, then `n` bulk strings, handle pipelining and partial reads.
  That is the natural "what would you build next" answer.

## 4. The data structures, and why each was chosen

**Strings** (`db.rs`): a plain `HashMap<String, String>`. O(1) get/set. A
second `HashMap<String, Instant>` holds expiry deadlines for keys set with a
TTL.

**Lists** (`RList`): a `VecDeque<String>`. The whole point of `LPUSH`/`LPOP`/
`RPUSH`/`RPOP` is O(1) operations at both ends, and `VecDeque` is exactly the
double-ended queue that gives you that. A `Vec` would make front operations
O(n). `LRANGE` is `iter().skip(start).take(end - start + 1)`.

**Sets** (`RSets`): a `HashSet<String>`. Membership test (`SISMEMBER`) is O(1)
on average, duplicates are rejected for free by `insert` returning `bool`.

**Sorted sets** (`RSortedSet`): the most interesting one, and worth the most
prep. It keeps *two* structures in sync:

- `members: HashMap<String, OrderedFloat<f64>>` for O(1) `ZSCORE` lookups and
  for finding a member's old score on update.
- `sorted: BTreeSet<SortedMembers>` keeping entries ordered by `(score, member)`
  so `ZRANGE` comes out in score order.

On `ZADD` with an existing member, the code removes the old `(member, old_score)`
entry from the tree before inserting the new one; a `BTreeSet` entry is
immutable, so "update" means remove-then-insert. `SortedMembers` implements
`Ord` manually: compare by score, then break ties by member name. Ties *must*
be broken by member, otherwise two members with the same score would compare
equal and one would silently fail to insert.

**Why `OrderedFloat`?** `f64` does not implement `Ord` in Rust because of NaN
(NaN is not equal to itself, so total ordering is impossible). `BTreeSet`
requires `Ord`. The `ordered-float` crate wraps `f64` and provides a total
order. That is a classic "why doesn't this compile?" Rust question, and having
hit it for real is a good story.

**The real Redis comparison:** real Redis sorted sets use a skiplist plus a
hash table, not a B-tree. A skiplist gives O(log n) insert/range with simpler
concurrent-read properties and powers `ZRANGEBYSCORE`-style queries. Saying
"I used a BTreeSet for simplicity; Redis itself uses a skiplist, which I'd
love to implement next" is a strong, honest answer.

## 5. Concurrency model

```rust
pub struct Database {
    db:         Arc<RwLock<HashMap<String, String>>>,
    expiry:     Arc<RwLock<HashMap<String, Instant>>>,
    list:       Arc<RwLock<HashMap<String, RList>>>,
    set:        Arc<RwLock<HashMap<String, RSets>>>,
    sorted_set: Arc<RwLock<HashMap<String, RSortedSet>>>,
}
```

- **One task per connection** via `tokio::spawn`. Tokio multiplexes thousands
  of these tasks onto a small pool of OS threads, so you get high concurrency
  without a thread per client.
- **`Arc`** lets every connection task share the same maps; `Database::clone()`
  clones the pointers, not the data.
- **`RwLock` (tokio's async one)** because reads dominate: many `GET`s can hold
  read locks concurrently, writers take exclusive locks. The lock is held
  across `.await`-free critical sections only, which is the correct way to use
  async locks.
- **One lock per data type**, not one global lock: a slow list operation does
  not block `GET`s on strings. This is deliberately coarse-grained (a whole
  type per lock) but finer than a single global lock, and that tradeoff is
  worth being able to narrate.

Likely follow-ups and honest answers:

- *"Real Redis is single-threaded, why did you go multi-threaded?"* Real Redis
  uses one event loop so it needs no locks at all, and its commands are fast
  enough that one core saturates the network. I wanted to learn Rust's shared
  state model (`Arc` + async `RwLock`), so I went concurrent. If I were
  copying Redis exactly, a single-threaded event loop would be simpler and
  remove every lock.
- *"What are the weaknesses?"* A long `ZRANGE` holds that type's lock; there's
  no sharding; a panicking command handler kills only its own connection task
  (tokio isolates task panics), but panics like `ttl.parse().expect(...)` on
  bad input still drop that client's connection instead of returning an error.

## 6. Expiry (TTL) and persistence

**Expiry exists. Persistence does not.** Do not claim persistence; the repo
has none.

- `SET key value EXP <seconds>` records `Instant::now() + ttl` in the expiry
  map.
- Expiration is **lazy/passive only**: `get()` calls `is_expired()`, and if the
  deadline has passed it deletes the key and returns `None`.
- Nothing else checks expiry, and there is no background sweeper, so an expired
  key that is never read again stays in memory. Real Redis combines passive
  expiry with an **active** cycle that periodically samples and deletes expired
  keys. That is the obvious next step, and a great answer to "how would you
  improve it."

For persistence, the honest answer when asked "does it persist?": no, it's
in-memory only; the next step would be AOF-style append-only logging of every
write command, and/or periodic RDB-style snapshots, which is exactly what Redis
itself does.

## 7. How to run and test it

```bash
cargo run          # starts on 127.0.0.1:7635
```

In another terminal, talk to it with netcat (not `redis-cli`, see section 3):

```bash
nc 127.0.0.1 7635
SET name ahmed
+OK
GET name
$5
ahmed
SET session abc EXP 10     # expires 10 seconds later
GET session                # after 10s: $-1
LPUSH queue job1
RPUSH queue job2
LRANGE 0 1 queue           # note: this build takes LRANGE <start> <end> <key>
SADD tags rust
SISMEMBER tags rust
ZADD 9.5 ahmed scores      # note: this build takes ZADD <score> <member> <key>
ZRANGE scores 0 10
ZSCORE scores ahmed
DEL name
```

There are no automated tests in the repo yet (`cargo test` has nothing to run).
Adding unit tests per data structure plus an integration test over TCP is the
first thing I'd add.

## 8. Known limitations (own them before the interviewer finds them)

Being able to list your own bugs unprompted is a senior move. From reading the
code:

1. **Request side is not RESP.** Inline commands only, so standard clients
   can't connect (section 3).
2. **`DEL` reply semantics are inverted/odd.** The code returns `$-1` when the
   key *was* deleted and `$-0` when it wasn't; Redis returns the integer count
   (`:1` / `:0`). This is a bug to fix.
3. **Argument order drift.** This build parses `LRANGE <start> <end> <key>` and
   `ZADD <score> <member> <key>`, while Redis (and this repo's README) use
   `LRANGE <key> <start> <stop>` and `ZADD <key> <score> <member>`. The parser
   and docs disagree and should be reconciled.
4. **README over-promises.** It lists `TTL` and `ZRANGE ... WITHSCORES`, which
   are not implemented.
5. **Panics on bad input.** `ttl.parse::<u64>().expect(...)` and `end - start`
   underflow (if `end < start` in `LRANGE`/`ZRANGE`, in debug builds) will
   panic the connection task instead of returning `-ERR`.
6. **`SISMEMBER` returns a non-standard string** (`$FOUND!` / `$NOT FOUND!`)
   instead of an integer.
7. **Expired keys linger** if never read again (passive expiry only).
8. **Hardcoded port, no config**, and `unwrap()` on bind/accept/write means a
   failed bind or a client reset kills the process/task rather than being
   handled.

If asked "what would you do next": RESP request parser, active expiry sweeper,
AOF persistence, fix the reply semantics above, integration tests, and a
skiplist for sorted sets.

## 9. Likely interview questions, with honest answers

**Q: Explain the project in 30 seconds.**
A Redis clone in Rust: an async TCP server on tokio that stores strings, lists,
sets, and sorted sets in memory, with TTL expiry on keys. Multiple clients
connect at once; each gets its own async task and they share the store behind
`Arc<RwLock<...>>`.

**Q: Why Rust for this?**
Memory safety without a GC, fearless concurrency (the borrow checker forces the
shared-state design to be correct at compile time), and performance close to C,
which is what Redis itself is written in. Also: I wanted to learn systems
programming, and reimplementing a known system is the fastest way.

**Q: Walk me through what happens when a client sends `SET a 1 EXP 60`.**
Accept loop spawns a task; `BufReader::read_line` gets the line;
`command_parser` splits it into `["SET","a","1","EXP","60"]` and matches the
five-element pattern; `db.set` takes a write lock on the string map, inserts
`a -> 1`, computes `Instant::now() + 60s` and stores it in the expiry map;
client gets `+OK\r\n`. On a later `GET a`, `is_expired` compares the stored
instant to now and lazily deletes the key if it lapsed.

**Q: How do two clients writing the same key behave?**
Writes serialize on that map's write lock, so there's no data race; last write
wins. The borrow checker plus `RwLock` is what makes that guarantee enforceable
at compile time rather than by convention.

**Q: Why `RwLock` and not `Mutex`?**
Reads (`GET`, `ZRANGE`, `ZSCORE`) far outnumber writes in a cache workload, and
`RwLock` lets them proceed in parallel. For the write-heavy paths it degrades
to `Mutex` behavior.

**Q: How is this different from real Redis?**
Single-threaded event loop vs my task-per-connection; full RESP vs my inline
parsing; active + passive expiry vs my passive-only; RDB/AOF persistence vs
none; skiplists vs my BTreeSet; and hundreds of commands vs my ~16. Mine is
the core loop of Redis rebuilt to understand it.

**Q: What was the hardest part?**
Two candidates, both true to the code: (a) the sorted set, keeping the
`HashMap` and `BTreeSet` consistent on update and discovering why `f64` can't
be `Ord`; (b) the shared-state async design, getting `Arc` clones of the
`Database` into spawned tasks with correct lifetimes. Pick whichever you can
talk about longest.

**Q: How would you scale it?**
Shard the maps (e.g., 16 shards keyed by hash of key) to reduce lock
contention; move to a proper RESP parser so connection handling can pipeline;
add active expiry; snapshot/AOF for durability. Beyond one process: consistent
hashing across nodes, like Redis Cluster.

## 10. "How much of this did AI write?"

Honest answer framework, adjust the details to what you actually remember:

> I designed it and drove every decision: the data types to support, choosing
> tokio and the task-per-connection model, the `Arc<RwLock<HashMap>>` per-type
> sharing, the dual HashMap+BTreeSet sorted set, and lazy expiry. I used AI as
> a pair programmer for Rust syntax and fighting the borrow checker, and for
> boilerplate like the command-match arms. The architecture, the tradeoffs, and
> the debugging were mine, which is why I can walk through every line here.

Only say the parts that are true. Before any interview, re-read this file next
to the code and make sure you can explain, without notes: the sorted-set update
path, why `OrderedFloat` exists, and why `VecDeque` for lists. Those three are
the places an interviewer will probe to check it's really yours.
