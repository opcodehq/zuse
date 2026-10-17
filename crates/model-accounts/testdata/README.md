These public, test-only fixtures exercise signed OAuth identity validation.
The private key is intentionally checked in so protocol tests are reproducible;
it has no production identity, account, or authorization. Its matching public
key is in `jwks.json`. Production code never loads these files.
