# Web error handling

AgentLens treats browser responses and server diagnostics as separate trust boundaries. An authenticated request that encounters an internal database or template failure receives HTTP 500 with the fixed plain-text body `Internal server error`. The underlying error is never rendered into the response.

The corresponding server event contains only fields selected by the application:

- the static operation that failed, such as `load sessions` or `render transcript`;
- a bounded error category, such as `pool_closed`, `database`, or `template`;
- an optional database code or I/O error kind, never the database error message;
- the HTTP status and request method.

HTTP trace spans intentionally omit the URI and headers. This prevents filter values in query strings and Basic authentication credentials from being copied into logs. Operators should use the operation, category, and safe code to diagnose the affected subsystem, then inspect local database and process health separately. Do not add arbitrary error display strings, request headers, request URIs, transcript values, or SQL parameter values to web logs.

Not-found and malformed-request responses remain distinct client errors. They do not render an internal error source.

## Verification

The normal test suite closes a synthetic database pool to exercise a real route failure. It verifies the exact generic response and captures diagnostics to confirm that useful allowlisted context remains while synthetic query content, credentials, authorization data, and an injected database error message do not appear. The injected source text makes the regression fail against the former behavior, which returned and logged the underlying error.

Run the complete project checks from the repository root:

```bash
bash scripts/check.sh
```

This change requires restarting a running AgentLens web process to load the new behavior. It does not change the schema or indexed data, so no migration or rebuild is required.
