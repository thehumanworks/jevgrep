# Synthetic semantic criteria
- [NAME] A function's name must describe its observable action: a function named as a read or preview must not mutate persistent state. Names of local variables alone are not evidence.
- [RESP] A data decoding function must only validate and transform input; it must not also send network messages or persist data. Calling a pure validation helper is allowed.
- [FAIL] A failed attempt to persist an update must remain observable to the caller; do not return a success value from a failure handler. Explicitly returning an error or propagating it is allowed.
