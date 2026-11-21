# Owly AirDrop source

Pinned source: `https://github.com/t4t5/owly`, v0.4.0,
`2e9b2b0228432d5e402a9c90a031b419960d0bd1`. The source, upstream tests and MIT
license are retained here; the standalone Cargo manifest disables workspace
inheritance. `UPSTREAM-README.md` contains the original crate overview.

The Windows integration uses these protocol modules and state machines through
`wayline-core`, which owns application callbacks, cancellation, board feedback,
identity storage and network workers. There is one HTTP/plist/archive/mDNS
implementation, in this crate.

Local adaptations validate compression and HTTP completion before publishing
files, expose bounded upload progress and optional validated DVZip completion,
support cooperative archive preparation cancellation, and use atomic Windows
no-replace publication with rollback on a failed commit or final response.
Multi-photo offers may list each file within nested archive wrapper directories.
The receiver validates that shape and saves each file under the name shown for
consent, following [Owly ASK-5 and UP-2a](https://github.com/t4t5/owly/blob/main/docs/SPEC.md#64-post-ask).
When distinct offered paths share a filename, each is staged separately and
published with the same collision suffixes as other received files.
Unlisted AppleDouble members associated with an offered file or its wrapper
count toward archive limits and are discarded before staging.
DVZip blocks are decoded through a fixed buffer with an independent chunk
limit and the existing upload and extracted-size limits.
Wayline accepts up to 65,536 archive entries for offers that include a
directory. The regular file-offer limit is 4,096. Both use a bounded path index.
Accepted responses requesting connection closure reconnect before upload;
transport failure/timeout remains distinct from an explicit refusal. Windows
regressions live in `crates/wayline-core/tests/airdrop_migration.rs`; upstream
library/vector tests run on Windows, while the veth harness requires Linux.
Keep source attribution and the MIT license in release packages.
