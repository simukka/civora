Security rules we need from day one

These are non-negotiable.

No native community code hot patches. Use Wasm.
No patch gets direct filesystem, network, wallet, clipboard, or OS access.
Player private keys never enter Wasm memory.
Every patch has a content hash, author signature, tests, and rollback plan.
Every governance change has a readable diff and delayed activation.
Clients can refuse patches locally.
The kernel must be small enough to audit.
Voting is not the default political system.
Every accepted patch must remain available from multiple peers before activation.