## What changed

<!-- One or two sentences. The subject line says the verb; this says the reason. -->

## Why

<!-- The failure or the measurement that motivated it. Link the issue. -->

## What you ran

- [ ] `./scripts/preflight.sh`
- [ ] `cargo test --workspace --lib --tests`
- [ ] `npm --prefix ui run build`
- [ ] `./scripts/app-e2e.py` (if the wheel, a dialog, or drag handling changed)

<!-- Paste the command and its result. "Passes" without the command is not a result. -->

## What you could not verify

<!-- Platform you did not run on, a session type you did not test, an input you lacked.
     This section is useful, not embarrassing. -->

## Invariants

- [ ] The source file is still read-only for this path
- [ ] The output is published atomically (temp dir, then rename)
- [ ] External tools are invoked with an argv array
- [ ] No new UI control owns a value another control also owns
- [ ] New bundled tool or asset is listed in `NOTICE` with a compatible licence
