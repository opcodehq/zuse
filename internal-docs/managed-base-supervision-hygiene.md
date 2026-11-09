# Clean supervision state before base capture

A restored clean Boat base exposed a saved `supervisor.json` PID before its new
service started. The corrected diagnostic later observed a healthy fresh resident,
stable socket inode, matching bound VFS inode and zero shutdown mask across the
actual GitHub CLI proof and separate exec calls. This establishes stale state in
the reusable filesystem; it does not establish the cause of the newer artifact's
separate connection failure.

The publisher now stops and cleans an idle builder before snapshot capture.
`publication-stop` is a separate, nonce-bound private RPC. It rejects task
bindings, runtime data, live children, recovery/repair history, unknown IPC files
and a primary PID1 supervisor. It cannot be used as a task pause/restart action.
The existing task lifecycle and Boxd boot command remain unchanged.

The builder helper verifies the exact runtime hash, credential-free managed-base
manifest, empty task/status/recovery directories and enabled systemd boot owner.
After the original resident exits, it acquires the released supervisor lock and
removes only verified-owned `supervisor.json`, `supervisor.lock` and `control.sock`
leaves. Directory descriptors and no-follow opens prevent traversal; symlinks,
hardlinks, unknown files, nonempty locks and unexpected record fields reject
capture. The boot unit stays enabled and its content hash stays unchanged.

Derivation from a previously verified clean parent has an explicit compatibility
step: stop its old idle service before installing the overlay. That path requires
the clean lineage/preflight, original resident identity, no task authority/data,
and the retained enabled unit. Final capture requires the new atomic RPC and
cannot use the compatibility receipt. A failed check prevents snapshot submission.

Native Boxd and Boat use their enabled service on fresh restore. Explicit OCI
Boxd images with a PID1 resident and unconditional restart policy are rejected at
this capture seam until a safe provider stop/capture contract is verified; the
publisher does not change their boot command or claim hygiene support.

Verification includes real subprocess RPC/exit/lock tests, refusal with a bound
live child and recovery/repair history, exact path ownership and alias tests,
fresh resident startup after cleanup, and publisher/lineage receipt ordering.
A new derived clean-parent publication and restored-resident proof remain
required before certifying or enabling a new artifact. Source tests alone are
not a provider certification.
