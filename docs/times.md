# Times

Cargo builds the code, then runs each test binary one at a time. In a binary, libtest runs the tests in parallel.

The totals line shows up to three times, for example:

```
4317 passed, 0 failed, 206 skipped in 12.27s (build 28.40s, total 53.00s)
```

- `Elapsed` in the tables, and the time after `in` on the totals line, is the time that libtest measured in each test binary. It does not count the build, the start of each binary, or the time that rustdoc needs before it runs doc tests.
- `build` is the time from cargo's `Finished` line.
- `total` is the time from the start of `cargo tparse` to the end of cargo's output, so it counts everything. It shows only when `cargo tparse` reads cargo's output live from a pipe, not for saved output. It does not show on Windows.
- With `--slow`, the time of each test can overlap with other tests, because tests run in parallel.
