# UI parity

Reference: swarajbachu/zuse `43e14c1` (main, fetched 2026-10-23). Scope is the UI for capabilities already implemented here. Pull request views and right-side panes are excluded. Existing unfinished local/cloud terminal changes are deferred and must be preserved.

Completion requires matching the reference composition, dimensions, typography, colors, states and interactions in native and browser builds. A compiling page or a screenshot alone does not establish workflow completion.

| Surface | Implemented | Verified | Remaining |
| --- | --- | --- | --- |
| Shared settings layout | Native 20px headings, 12px/11px rows, 11px buttons, authored light frame fill, Geist default and compact rail; shared cloud tabs | Native check; compiled browser at 1280px and 760px; reference component measurements | Remaining page composition, account switcher and signed-in states |
| General/account | Sign-in/out, theme, notification preferences | Existing isolated account checks | Account states, display, theme and preference parity |
| Providers/defaults | Install, enable, login, models and account actions | Existing fixtures | Page structure, compact controls and expanded rows |
| Plugins/remote access | Catalog, MCP setup, pairing, managed access | Existing isolated workflows | Reference settings composition and states |
| Cloud settings | Shared Setup/Usage navigation; repositories/images separated from billing/credits | Real engine/browser navigation; native check | Reference hosting/subscription panels, signed-out/loading/error states and signed-in comparison |
| Composer/chat creation | Shared 106px empty card, 768px landing and 896px conversation widths, 12px/18px input, content padding and matching route floors; conditional landing copy | Browser pixel bounds verified at 1280×1000; native check | Model/destination/worktree/import pickers, local/cloud creation and all notification trays |
| Sidebar/navigation | Threads, project groups and conversation tabs | Previous build checks | Density, menus, account and usage entry points |
| Archive | Browser local/cloud list, filtering, project labels, saved preview, back navigation and restore through existing lifecycle commands | Real engine/browser filter → restore chosen thread → preview remaining archive → return; zero provider turns | Project archive entry points, reference list/preview presentation and native/cloud full comparison |
| Usage | Provider account usage and cloud usage history | Existing isolated data checks | Reference dashboard, meters and account controls |
| Native-only preferences | Appearance, voice, files, appshots, shortcuts | Existing native checks | Keep implemented controls reachable while aligning presentation |

Evidence from this pass will be recorded per verified chunk. Existing fixture results are not live account or payment acceptance.

## Verified chunk: composer geometry, settings navigation and browser archives

- Reference component preview uses the actual settings-panel, frame, card, switch and segmented-tabs modules at the pinned commit. Measured 768px settings column, 12px frame radius, 10px card radius, 4px frame padding, 12px/11px row text and 224×28px cloud tabs. The reference overrides Tailwind radius defaults.
- `scripts/acceptance/ui-parity.mjs` runs the compiled browser against a disposable real engine. It checks the composer outline pixels at 768×106px, filters two archived threads, restores only the chosen thread, opens saved history, returns to the list, switches Cloud tabs and checks compact settings. Keyboard navigation transfers focus to the selected tab. It asserts no provider turn or checkout request.
- Evidence: `.context/ui-parity-acceptance.json`, `ui-parity-browser-final-acceptance.log`, `ui-parity-native-final-verified-check.log`, and `ui-parity-web-final-verified-build.log`. Screenshots are `.context/ui-parity-{composer,archive,archive-preview,cloud-setup,cloud-usage,settings-compact}.png`.
- Cloud screenshots in this first pass are from an isolated signed-out account. Navigation is verified; subscription, payment and signed-in image states still need their own acceptance. Full UI parity is not complete.
