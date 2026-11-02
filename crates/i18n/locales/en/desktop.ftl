# Imported from Zuse desktop catalogs. See migration.json for provenance.
# Edit individual translations here; do not regenerate this file.

# Source key: desktop:checking_for_updates
desktop-checking_for_updates = Checking for Updates…

# Source key: desktop:download_update_v
# Variables: version
desktop-download_update_v = Download Update (v{ $version })…

# Source key: desktop:downloading_update
# Variables: percent
desktop-downloading_update = Downloading Update…{ " " }{ $percent }%

# Source key: desktop:install_update_restart_v
# Variables: version
desktop-install_update_restart_v = Install Update & Restart (v{ $version })

# Source key: desktop:retry_update_check
desktop-retry_update_check = Retry Update Check

# Source key: desktop:check_for_updates_up_to_date
desktop-check_for_updates_up_to_date = Check for Updates… (Up to date)

# Source key: desktop:check_for_updates
desktop-check_for_updates = Check for Updates…

# Source key: desktop:file
desktop-file = File

# Source key: desktop:new_chat
desktop-new_chat = New Chat

# Source key: desktop:open_project
desktop-open_project = Open Project…

# Source key: desktop:close_tab
desktop-close_tab = Close Tab

# Source key: desktop:settings
desktop-settings = Settings…

# Source key: desktop:view
desktop-view = View

# Source key: desktop:toggle_sidebar
desktop-toggle_sidebar = Toggle Sidebar

# Source key: desktop:toggle_files_pane
desktop-toggle_files_pane = Toggle Files Pane

# Source key: desktop:toggle_terminal
desktop-toggle_terminal = Toggle Terminal

# Source key: desktop:focus_composer
desktop-focus_composer = Focus Composer

# Source key: desktop:developer
desktop-developer = Developer

# Source key: desktop:reload
desktop-reload = Reload

# Source key: desktop:force_reload
desktop-force_reload = Force Reload

# Source key: desktop:toggle_developer_tools
desktop-toggle_developer_tools = Toggle Developer Tools

# Source key: desktop:window
desktop-window = Window

# Source key: desktop:view_changelog
desktop-view_changelog = View Changelog

# Source key: desktop:report_a_bug
desktop-report_a_bug = Report a Bug

# Source key: desktop:export_diagnostics_for_bug_report
desktop-export_diagnostics_for_bug_report = Export Diagnostics for Bug Report...

# Source key: desktop:zuse_on_github
desktop-zuse_on_github = Zuse on GitHub

# Source key: desktop:pairingTitle
desktop-pairingTitle = Phone wants to connect

# Source key: desktop:pairingBody
# Variables: deviceIdentifier, deviceLabel
desktop-pairingBody = { $deviceLabel }{ " " }· Device{ " " }{ $deviceIdentifier }

# Source key: desktop:quitTitle
desktop-quitTitle = Quit Zuse (Beta)?

# Source key: desktop:quitAnyway
desktop-quitAnyway = Quit anyway

# Source key: desktop:quitWhenIdle
desktop-quitWhenIdle = Quit when idle

# Source key: desktop:quitDetail
desktop-quitDetail = Quitting now will stop them mid-turn. You can quit anyway, or have Zuse quit automatically once they finish.

# Source key: desktop:runningAgents
# Variables: count
desktop-runningAgents = { $count }{ " " }agents are running currently.

# Source key: desktop:runningAgents_one
# Variables: count
desktop-runningAgents_one = { $count }{ " " }agent is running currently.

# Source key: desktop:runningAgents_other
# Variables: count
desktop-runningAgents_other = { $count }{ " " }agents are running currently.

# Complete plural message. $count is numeric; remaining variables retain their source names.
desktop-runningAgents-plural =
    { $count ->
       [one] { $count }{ " " }agent is running currently.
       *[other] { $count }{ " " }agents are running currently.
    }
