-- 'computer-request' change hints: the API adds one after a computer request
-- of a chat is made, decided, claimed, finished or stopped, so the owner's
-- apps show (or clear) its approval at once instead of on their next poll.
--
-- Like the other change hints it carries only the chat (resource) and its
-- computer (workspace), never command text or output; apps refetch through
-- the authorized reads. The API only forwards this kind to sockets whose
-- ticket asked for it. Only API versions that filter it write it, so an older
-- API (before or after a rollback) never sends it to clients that would
-- reject it.
ALTER TABLE zuse_rust_account_event_outbox
 DROP CONSTRAINT zuse_rust_account_event_outbox_kind_check;
ALTER TABLE zuse_rust_account_event_outbox
 ADD CONSTRAINT zuse_rust_account_event_outbox_kind_check
 CHECK(kind IN ('workspace','conversation','connection','authentication','github','environment','task-command-queue','task-command-progress','machine-image','billing','computer-request'));
