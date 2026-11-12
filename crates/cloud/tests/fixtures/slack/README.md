Sanitized Slack Events API request-byte fixtures for the upstream app's mention,
new DM and mentioned thread follow-up scenarios. Tests sign the exact file bytes
before sending them through the HTTP router, including Slack retry headers.

These are synthetic wire recordings based on the original test scenarios in
`infra/api/test/slack/app.test.ts`, not recordings from a live installation.
IDs, timestamps, text and the deprecated verification token are synthetic; no
production credentials or customer messages are included.

`official-hmac.form` is the public verification vector from
[Slack's request verification documentation](https://docs.slack.dev/authentication/verifying-requests-from-slack/).
The corresponding test uses its published example secret, timestamp and signature;
it does not use the local test signer to generate the expected result. The form
payload only tests raw-byte signature compatibility, not a slash-command route.
