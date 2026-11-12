In-machine timing scripts for `provider_lifecycle_probe --scripts DIR`. The probe
uploads `common.sh` before each step and runs `fresh.sh` after the installed
runtime answers, `before.sh` before sleep and `resume.sh` after resume. Each
prints one JSON line. To measure the guest's agent warm-up, copy
`fresh-warm.sh` over `fresh.sh`. Set `PROBE_SLEEP_WAIT_SECS=660` to resume after
Boat's ten-minute warm window, which forces its cold restore.
