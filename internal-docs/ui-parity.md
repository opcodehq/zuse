# UI parity

Reference: swarajbachu/zuse `43e14c1` (main, fetched 2026-10-23). Scope is the UI for capabilities already implemented here. Pull request views and right-side panes are excluded. Existing unfinished local/cloud terminal changes are deferred and must be preserved.

Completion requires matching the reference composition, dimensions, typography, colors, states and interactions in native and browser builds. A compiling page or a screenshot alone does not establish workflow completion.

| Surface | Implemented | Verified | Remaining |
| --- | --- | --- | --- |
| Shared settings layout | Native 20px headings, 12px/11px rows, 11px buttons, authored light frame fill, Geist default and compact rail; shared cloud tabs | Native check; compiled browser at 1280px and 760px; reference component measurements | Remaining page composition, account switcher and signed-in states |
| General/account | Sign-in/out, theme, notification preferences | Existing isolated account checks | Account states, display, theme and preference parity |
| Providers/defaults | Install, enable, login, models and account actions | Existing fixtures | Page structure, compact controls and expanded rows |
| Plugins | Connected list and independent search, Add menu, separate catalog, shared details and account controls, modal remote MCP setup | Compiled browser workflow fixture; native check; dark screenshots; keyboard focus and credential submission | Reference inline API-key entry, pending and confirmation dialogs, action-menu keyboard behavior, native pixel comparison and signed-out consistency |
| Remote access | Pairing and managed access | Existing isolated workflows | Reference settings composition and states |
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


## Verified chunk: plugin settings, discovery, details and setup

- Browser settings now separate connected plugins from discovery, with a 192px installed search, anchored Add menu, connected frame and per-connection toggle/disconnect actions. Discovery retains the sidebar, uses the 768px outer column, and opens details from row titles. Its search is independent of settings search.
- Native and browser share the detail composition, account rows, Information section and provider/custom-server actions. Connecting another account reserves existing labels, including disabled connections, and chooses the next default such as “Figma 2”. Account changes clear the selected detail and ephemeral credential form.
- Shared remote MCP setup uses a 448px modal, 28px fields, segmented authentication controls, masked credentials, validation, Escape, Tab wrapping and Enter submission. Explicit GPUI focus handles must be marked as tab stops; the same correction applies to the shared Cloud tabs. The implemented backend supports remote MCP setup; no placeholder local-transport control was added.
- `scripts/acceptance/plugin-settings.mjs` drives the compiled GPUI browser against a disposable engine and an isolated plugin/account protocol fixture. It checks selected toggling, independent filtering, catalog/details navigation, additional-account naming, poll reconciliation, confirmed selected disconnect, one API-key submission on Enter, MCP setup with keyboard authentication selection, account replacement and read-only write guards. It also checks that authentication labels actually paint. All passed with zero provider turns or checkout requests. This is not live account/OAuth acceptance.
- Evidence: `.context/plugin-parity-acceptance.json`, `plugin-parity-browser-acceptance.log`, `plugin-parity-native-final-check.log`, `plugin-parity-web-final-build.log`, and rendered `plugin-parity-*.png`. The existing composer/archive/Cloud-tab regression also passed in `plugin-parity-regression-acceptance.log`.
- Remaining plugin differences include the reference’s inline API-key entry and confirmation/pending presentation, action-menu keyboard polish, signed-out native/catalog consistency and native screenshots. This chunk does not complete full UI parity.

## Account/cloud integration finding

The general account and plugin flows use the engine’s `Account` session, while cloud workspace and billing RPCs still authorize through the separate workspace `Auth` session. A local account sign-in therefore cannot currently authorize those cloud requests. The signed-out cloud page can stay in loading/error states. Resolve the identity integration and verify the real sign-in → cloud-settings/chat-creation path; a new sign-in button alone would not fix it. No terminal work was changed to address this finding.
