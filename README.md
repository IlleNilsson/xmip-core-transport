# xmip-core-transport

Direction-neutral Transport contracts for moving immutable Streams between Xmip and endpoints.

Receive and Send own orchestration, ports, groups, and locations. Transport owns byte movement and resource claims; it does not own authentication, message representation, contract evaluation, or operation semantics.

The current crate contains the common Transport boundary and initial File, TCP, UDP, HTTP, and SMTP implementations. Each technology can move into its declared child repository when it gains an independent consumer or release cadence.

Status: planned, with the common contract and initial implementations already present.
