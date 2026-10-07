/**
 * Localisation.
 *
 * One dictionary per language, one `t()` for lookup, and no translation strings
 * living in components. The earlier code had English scattered through JSX, which
 * made a second language impossible to add without touching every screen and
 * guaranteed that any string added later would be missed.
 *
 * The dictionary is typed against the English one, so a missing or misspelled key
 * in another language is a compile error rather than a silent fallback to
 * English at runtime.
 */

import type { ApiError } from "./api";

export const LANGUAGES = ["en", "id"] as const;
export type Language = (typeof LANGUAGES)[number];

/** What each language calls itself, for the picker. */
export const LANGUAGE_NAMES: Record<Language, string> = {
  en: "English",
  id: "Bahasa Indonesia",
};

export function isLanguage(value: unknown): value is Language {
  return typeof value === "string" && (LANGUAGES as readonly string[]).includes(value);
}

/**
 * The English dictionary, which defines every key the app may use.
 *
 * `en` is the source of truth for the key set. Placeholders are `{name}` and are
 * substituted by `t()`.
 */
const en = {
  // ---- navigation and shell -------------------------------------------
  "nav.overview": "Overview",
  "nav.providers": "Providers",
  "nav.playground": "Playground",
  "nav.history": "History",
  "nav.settings": "Settings",
  "app.tagline": "Local configuration bridge",
  "app.noTelemetry": "No telemetry",

  // ---- common actions --------------------------------------------------
  "action.cancel": "Cancel",
  "action.delete": "Delete",
  "action.edit": "Edit",
  "action.duplicate": "Duplicate",
  "action.test": "Test",
  "action.activate": "Activate",
  "action.active": "Active",
  "action.refresh": "Refresh",
  "action.retry": "Try again",
  "action.browse": "Browse...",
  "action.open": "Open",
  "action.add": "Add",
  "action.remove": "Remove",
  "action.done": "Done",
  "action.hide": "Hide",
  "action.copyDiagnostics": "Copy diagnostics",
  "action.openFolder": "Open folder",
  "action.clearLogs": "Clear logs",

  // ---- statuses --------------------------------------------------------
  "status.notTested": "Not tested",
  "status.testing": "Testing...",
  "status.compatible": "Compatible",
  "status.partiallyCompatible": "Partially compatible",
  "status.incompatible": "Incompatible",
  "status.notDetermined": "Not determined",
  "status.testFailed": "Test failed",
  "status.detected": "Detected",
  "status.notFound": "Not found",
  "status.configOk": "Config OK",
  "status.configUnreadable": "Config unreadable",
  "status.noConfigYet": "No config yet",
  "status.externalConfiguration": "External configuration",

  // ---- overview --------------------------------------------------------
  "home.title": "Overview",
  "home.refresh": "Refresh",
  "home.noAgents": "No coding agent detected",
  "home.installGuide": "Installation guide",
  "home.configure": "Configure",
  "home.playground": "Playground",
  "home.history": "History",
  "home.version": "Version",
  "home.versionValue": "Version {version}",
  "home.versionUnknown": "unknown",
  "home.notInstalled": "Not installed",
  "home.openAgent": "Open {agent}",
  "home.checkingSetup": "Checking your setup...",
  "home.intro":
    "Eclyzer Bridge points Codex and Claude Code at the provider you choose, without you editing configuration files by hand.",
  "home.setUpProvider": "Set up a provider",
  "home.noAgentsLong":
    "Eclyzer Bridge could not find Codex or Claude Code on this machine. Install one, then choose Refresh.",
  "home.notFoundTitle": "{agent} was not found",
  "home.notFoundBody":
    "Eclyzer Bridge installs nothing on your behalf. Install {agent}, then choose Refresh.",
  "home.invalidConfigTitle": "Eclyzer Bridge will not modify this configuration",
  "home.invalidConfigBody":
    "{path} is not valid. Eclyzer Bridge refuses to rewrite a file it cannot parse, because doing so would destroy settings it does not understand. Fix the file yourself, then choose Refresh.",
  "home.activeFallback": "A provider profile",
  "home.activeManagedHint": "set by Eclyzer Bridge",
  "home.externalHint": "your own settings; Eclyzer Bridge did not write these",
  "home.defaultConfiguration": "Default configuration",
  "home.defaultHint": "no custom provider configured",
  "home.lastTested": "Last tested",
  "home.configFile": "Config file",
  "home.refreshAgent": "Refresh {agent} detection",
  "home.openLinkFailed": "Could not open the link.",

  // ---- launch dialog ---------------------------------------------------
  "launch.title": "Open {agent}",
  "launch.folderLabel": "Project folder",
  "launch.folderHint":
    "{agent} starts in this folder. Eclyzer Bridge passes it as a single argument, so a folder name containing spaces or quotes is safe.",
  "launch.preview": "Preview command",
  "launch.previewTitle": "Command that will run",
  "launch.previewHint":
    "The model and provider names are passed as separate arguments, never interpreted as shell commands.",
  "launch.body":
    "{agent} opens in a new terminal window in the folder you choose. Eclyzer Bridge exits as soon as the window opens, so closing Bridge never closes your agent.",
  "launch.openedToast": "Opened in a new terminal window.",
  "launch.folderPickerFailed": "Could not open the folder picker.",

  // ---- providers -------------------------------------------------------
  "providers.title": "Providers",
  "providers.subtitle":
    "A provider profile is an endpoint, a model, and credentials. Eclyzer Bridge writes only the settings it owns and leaves everything else in your configuration untouched.",
  "providers.add": "Add provider",
  "providers.empty": "No providers yet",
  "providers.emptyBody": "Add a provider to point {agent} at an endpoint you choose.",
  "providers.testFirst": "Test this provider against the agent first.",
  "providers.addKeyFirst": "Add an API key before activating.",
  "providers.noKey": "No API key",
  "providers.model": "Model",
  "providers.targets": "Targets",
  "providers.nothing": "nothing",
  "providers.tested": "tested {when}",
  "providers.loading": "Loading your providers...",
  "providers.yourAgent": "your agent",
  "providers.noAgentsBody": "Install Codex or Claude Code, then reload this page.",
  "providers.costNoticeTitle": "Testing uses your provider account",
  "providers.costNoticeBody":
    "Test Connection and the Playground send real requests to your provider and may count towards your usage or cost. Eclyzer Bridge never sends anything on its own.",
  "providers.costNoticeAck": "Got it",
  "providers.testAria": "Test {name} against {agent}",
  "providers.duplicateAria": "Duplicate {name}",
  "providers.deleteAria": "Delete {name}",

  // ---- provider editor -------------------------------------------------
  "editor.createTitle": "Add a provider",
  "editor.baseUrl": "Base URL",
  "editor.apiKey": "API key",
  "editor.apiKeyKeep": "Leave blank to keep the stored key.",
  "editor.apiKeyHint":
    "Sealed with Windows DPAPI for your account. It is never written to a profile file, a log, or a backup.",
  "editor.clearKey": "Remove the stored API key",
  "editor.model": "Model ID",
  "editor.targets": "Use with",
  "editor.notes": "Notes",
  "editor.tabGeneral": "General",
  "editor.tabAdvanced": "Advanced",
  "editor.authMode": "How the key is sent",
  "editor.authModeBearer": "Authorization: Bearer (most providers)",
  "editor.authModeXApiKey": "x-api-key (Anthropic style)",
  "editor.timeout": "Request timeout (ms)",
  "editor.headers": "Extra headers",
  "editor.headersHint":
    "Values are stored in plain text in your profile and written into the agent's own configuration file. Put your gateway credential in the API key field instead, where it is sealed with Windows DPAPI. Authorization, x-api-key, and proxy-authorization cannot be overridden here.",
  "editor.headerName": "Header name",
  "editor.headerValue": "Value",
  "editor.queryParams": "Query parameters",
  "editor.reset": "Reset advanced options",
  "editor.copyEndpoint": "Copy endpoint and model",
  "editor.insecure": "I understand, send my key over plain HTTP",
  "editor.editTitleName": "Edit {name}",
  "editor.saveChanges": "Save changes",
  "editor.tabsAria": "Provider settings",
  "editor.presetLabel": "Start from a preset",
  "editor.presetHint":
    "Presets fill in a known endpoint and model. You can change everything afterwards.",
  "editor.chooseProvider": "Choose a provider...",
  "editor.profileName": "Profile name",
  "editor.profileNamePlaceholder": "DeepSeek for work",
  "editor.providerName": "Provider name",
  "editor.providerNameHint": "Shown in your agent's configuration so you can tell providers apart.",
  "editor.baseUrlHintApi":
    "The OpenAI-compatible or Anthropic-compatible API root. Do not include /chat/completions.",
  "editor.modelHintExact":
    "Passed to your agent exactly as written. It is never interpreted as a command.",
  "editor.keyStored": "A key is already stored ({fingerprint}). Leave this empty to keep it.",
  "editor.fingerprintStored": "stored",
  "editor.noKeyStored": "No key is stored yet. Enter one to activate this provider.",
  "editor.clearKeyHint":
    "The provider stays, but it cannot be activated until a new key is entered.",
  "editor.targetsHintProtocols":
    "Codex and Claude Code speak different protocols. Each is tested separately.",
  "editor.insecureTitle": "Insecure Endpoint",
  "editor.insecureBody":
    "Your API key may be sent without encryption. The endpoint below uses plain HTTP and is not on this machine.",
  "editor.insecureHint": "Only do this for an endpoint you control on a trusted network.",
  "editor.advancedBody":
    "These options exist for gateways that need extra headers or a different authentication style. Most providers need none of them.",
  "editor.authModeHint":
    "Bearer sends Authorization: Bearer <key>. x-api-key matches the Anthropic convention.",
  "editor.timeoutHint": "Milliseconds. Slow gateways may need longer; the default is 20000.",
  "editor.noHeaders": "No extra headers.",
  "editor.noQueryParams": "No extra query parameters.",
  "editor.valueHidden": "•••••• (value hidden)",
  "editor.emptyValue": "(empty)",
  "editor.removeHeaderAria": "Remove header {name}",
  "editor.headerValueAria": "Header value",
  "editor.removeParamAria": "Remove parameter {name}",
  "editor.modelAliasLabel": "Model alias",
  "editor.modelAliasNote":
    "Optional. Written to your agent as the model name when the provider expects a different ID than the one you test with.",
  "editor.modelAliasPlaceholder": "same as model ID",
  "editor.notesPlaceholder": "Anything you want to remember about this provider.",
  "editor.noEndpoint": "No endpoint set",
  "editor.errorName": "Give this provider a name you will recognize.",
  "editor.errorProviderName": "Enter the provider name.",
  "editor.errorBaseUrl": "Enter the endpoint URL.",
  "editor.errorBaseUrlScheme": "The URL must start with http:// or https://.",
  "editor.errorModel": "Enter the model ID exactly as your provider expects it.",
  "editor.errorTargets": "Choose at least one agent.",
  "editor.errorApiKey": "Enter the API key for this provider.",
  "editor.errorApiKeyShort": "That key looks too short. Check that you pasted all of it.",
  "editor.errorInsecure":
    "This endpoint uses plain HTTP. Tick the confirmation below, or use an https:// URL.",

  // ---- delete ----------------------------------------------------------
  "delete.title": "Delete {name}?",
  "delete.body":
    "The profile and its stored API key are removed. Your agent configuration is not changed.",
  "delete.anyway": "Delete anyway",
  "delete.confirm": "Delete provider",
  "delete.impactFailed": "Could not check whether this provider is active.",
  "delete.activeTitle": "This provider is currently active",
  "delete.activeBody":
    "{agent} is using {name}. Deleting it now would leave that agent pointed at a configuration Eclyzer Bridge no longer manages.",
  "delete.anAgent": "An agent",
  "delete.agentJoin": " and ",
  "delete.saferOrder":
    "The safer order is: activate another provider, or restore the default configuration, and then delete this one.",
  "delete.switchFirstTitle": "Choose another provider or restore the default configuration first.",
  "delete.keepAndSwitch": "Keep it - let me switch first",

  // ---- test results ----------------------------------------------------
  "test.title": "Test result",
  "test.passed": "Passed",
  "test.failed": "Failed",
  "test.warning": "Warning",
  "test.skipped": "Skipped",
  "test.copyResults": "Copy results",
  "test.summary": "{passed} of {total} checks passed",
  "test.reason": "Reason",
  "test.totalTime": "Total time",

  // ---- history and recovery -------------------------------------------
  "history.title": "History",
  "history.subtitle":
    "Every change Eclyzer Bridge makes is reversible, and every action here says what it will do before it does it.",
  "history.tabChanges": "Changes",
  "history.tabSnapshots": "Snapshots",
  "history.tabRecovery": "Restore",
  "history.viewLabel": "History view",
  "history.noAgentsBody": "History appears here once Eclyzer Bridge can see Codex or Claude Code.",
  "history.empty": "No changes yet",
  "history.emptyBody": "Changes appear here once Eclyzer Bridge modifies an agent's configuration.",
  "history.loadingChanges": "Loading change history...",
  "history.tableCaption": "Changes Eclyzer Bridge has made to this agent",
  "history.when": "When",
  "history.what": "Change",
  "history.result": "Result",
  "history.undoable": "Undoable",
  "history.succeeded": "Succeeded",
  "history.failed": "Failed",
  "history.yes": "Yes",
  "history.restore": "Restore",
  "history.loadingSnapshots": "Loading snapshots...",
  "history.noSnapshots": "No snapshots yet",
  "history.noSnapshotsBody":
    "A snapshot is saved before every change, so there is always something to return to. None have been taken for this agent.",
  "history.newestKept": "The newest 20 snapshots are kept",
  "history.newestKeptBody":
    "Older ones are removed automatically. Your original configuration is kept separately and is never deleted.",
  "history.undoTitle": "Undo the last change",
  "history.undoBody": "Returns this agent to the state just before its most recent change.",
  "history.undoNothing": " Nothing to undo right now.",
  "history.undoReversible": " The current state is snapshotted first, so you can undo the undo.",
  "history.undoButton": "Undo last change",
  "history.restoreDefaultTitle": "Restore what Eclyzer Bridge changed",
  "history.restoreDefaultBody":
    "Removes the provider Eclyzer Bridge added, returning this agent to its own default. Everything else in your configuration is left untouched. Settings you configured yourself are kept, even if they look like something Eclyzer Bridge would write.",
  "history.restoreDefaultButton": "Restore default",
  "history.restoreOriginalTitle": "Restore the original configuration",
  "history.restoreOriginalBody":
    "Puts back the exact file that existed before Eclyzer Bridge ever touched this agent. This is a historical restore, not a factory reset: if that file already contained your own gateway URL, model, or aliases, those values come back.",
  "history.restoreOriginalUnavailable":
    "Available once Eclyzer Bridge has changed this agent at least once.",
  "history.restoreOriginalButton": "Restore original",
  "history.checkingRecovery": "Checking recovery options...",
  "history.recoveryUnreadable": "Recovery options could not be read.",
  "history.currently": "Currently",
  "history.currentManaged": "Eclyzer Bridge configuration is active",
  "history.originalBackup": "Original backup",
  "history.lastChange": "Last change",
  "history.externalTitle": "Your configuration was not written by Eclyzer Bridge",
  "history.externalBody":
    "This agent is using a custom configuration Eclyzer Bridge did not create. Restoring the default would replace it, so the option below is offered with that in mind.",
  "history.resetTitle": "Reset {agent} to stock defaults",
  "history.resetBody":
    "Removes every custom provider and model override, including any you configured yourself before using Eclyzer Bridge, so {agent} goes back to its normal built-in configuration. Your login, sessions, MCP configuration, permissions, and unrelated settings are kept.",
  "history.resetButton": "Reset to stock defaults...",
  "history.resetConfirmTitle": "Reset {agent} to stock defaults?",
  "history.resetConfirmBody":
    "This removes custom model and provider overrides, including settings not created by Eclyzer Bridge. Your login, sessions, MCP configuration, permissions, and unrelated settings will be kept.",
  "history.resetWillRemove": "These settings will be removed:",
  "history.resetNothing":
    "No provider or model overrides were found, so this changes nothing. You can still run it to be sure.",
  "history.resetSnapshot":
    "A snapshot is taken first, so you can undo this or restore the original configuration afterwards.",
  "history.resetAction": "Reset {agent}",
  "history.resetGroupProvider": "Provider and credential",
  "history.resetGroupAliases": "Model aliases",
  "history.resetGroupModel": "Model selection",
  "history.managedWarning":
    "Your organization manages this setting at {paths}. Eclyzer Bridge will not change it, so a reset may not take effect. Ask your administrator.",
  "history.originalSaved": "Saved: the state before Eclyzer Bridge first changed anything.",
  "history.originalMissing": "Not saved yet.",
  "history.confirmRestoreSnapshot": "Restore the snapshot from {when}?",
  "history.confirmRestoreSnapshotBody":
    'This replaces your current {agent} configuration with the state saved before "{label}".',
  "history.confirmRestoreDefault": "Restore the default configuration?",
  "history.confirmRestoreDefaultBody":
    "The provider Eclyzer Bridge added is removed. Your other settings stay as they are.",
  "history.confirmRestoreOriginal": "Restore the original configuration?",
  "history.confirmRestoreOriginalBody":
    "Your configuration file is replaced with the copy saved before Eclyzer Bridge first changed it. Any changes you made since then are lost.",
  "history.restored": "Configuration restored.",
  "history.nowActive": "Now active",
  "history.snapshotTaken": "Snapshot taken",
  "history.keysUpdated": "Keys updated",
  "history.snapshotFirstTitle": "A snapshot is taken first",
  "history.snapshotFirstBody": "Whatever is there now is saved, so you can undo this.",
  "history.reversibleNote":
    "The state you just replaced was snapshotted, so this is reversible from the Snapshots list.",
  "history.restoredBody": "{agent} configuration has been written and verified.",

  // ---- playground ------------------------------------------------------
  "playground.title": "Playground",
  "playground.subtitle": "Send one prompt to a provider and read the raw reply. Nothing is stored.",
  "playground.provider": "Provider",
  "playground.model": "Model",
  "playground.endpoint": "Endpoint",
  "playground.prompt": "Prompt",
  "playground.send": "Send",
  "playground.sending": "Sending...",
  "playground.agentTabsAria": "Coding agent",
  "playground.billableTitle": "This request is billable",
  "playground.billableBody":
    "The prompt below is sent to your provider and may count towards your usage or cost. Nothing is sent unless you press Send.",
  "playground.noUsableProvider": "No usable provider for this agent",
  "playground.noUsableProviderBody":
    "Add a provider for {agent} and enter an API key, then come back.",
  "playground.formTitle": "Send a prompt",
  "playground.promptHint": "One turn only. The provider's reply is shown exactly as it arrives.",
  "playground.askShort": "Reply with the single word: ready",
  "playground.waiting": "Waiting for the provider...",
  "playground.requestFailed": "The request failed.",
  "playground.responseReceived": "Response received",
  "playground.streamed": "Streamed",
  "playground.transport": "Transport",
  "playground.technicalDetail": "Technical detail",
  "playground.tokensInline": "{input} in / {output} out tokens",

  // ---- settings --------------------------------------------------------
  "settings.title": "Settings",
  "settings.subtitle":
    "Local preferences. Nothing here is sent anywhere, and nothing here contacts a server.",
  "settings.language": "Language",
  "settings.languageHint": "Applies immediately and is remembered.",
  "settings.updates": "Updates",
  "settings.updateCheck": "Check for updates automatically",
  "settings.checkNow": "Check now",
  "settings.checking": "Checking...",
  "settings.updateInstall": "Download and install",
  "settings.openProjectPage": "Project page",
  "settings.generalSection": "General",
  "settings.aboutSection": "About",
  "settings.updateCheckOn": "Eclyzer Bridge will check GitHub for updates.",
  "settings.updateCheckOff": "Update checks are off.",
  "settings.updateCheckBody":
    "At most once every 24 hours, from GitHub Releases. No information about you or your machine is sent; this is an ordinary download request. Updates are never installed without your confirmation.",
  "settings.rememberFolderBody":
    "Used to suggest a folder when you open a coding agent. Turning this off forgets the saved path immediately.",
  "settings.logLevelBody":
    "Logs stay on this machine and are pruned after 7 days. Higher levels are useful when reporting a bug.",
  "settings.logLevelError": "Error - problems only",
  "settings.logLevelWarn": "Warn",
  "settings.logLevelInfo": "Info - recommended",
  "settings.logLevelDebug": "Debug",
  "settings.logLevelTrace": "Trace - very verbose",
  "settings.logLevelSaved": "Log level saved.",
  "settings.protectedBy": "Protected by",
  "settings.storedApiKeys": "Stored API keys",
  "settings.none": "none",
  "settings.providerProfiles": "Provider profiles",
  "settings.credentialHelper": "Credential helper",
  "settings.credentialHelperAvailable":
    "Available - Codex reads keys without them being written to config",
  "settings.credentialHelperNotFound": "Not found - Codex uses an environment variable instead",
  "settings.dataFolder": "Data folder",
  "settings.couldNotOpenDataFolder": "Could not open the data folder.",
  "settings.couldNotOpenBrowser": "Could not open your browser.",
  "settings.lastChecked": "Last checked {when}",
  "settings.notCheckedYet": "Not checked yet",
  "settings.updateCheckIncomplete": "The update check did not complete",
  "settings.upToDateTitle": "You are up to date",
  "settings.latestRelease": "Eclyzer Bridge {version} is the latest release.",
  "settings.updateAvailableTitle": "Version {version} is available",
  "settings.updateBody":
    "You are running {current}. Updates are downloaded from GitHub Releases and verified against a signature before anything is replaced.",
  "settings.releaseNotes": "Release notes",
  "settings.restartOnInstall": "Eclyzer Bridge restarts when the install finishes.",
  "settings.diagnosticsBody": "A summary of your setup you can paste into a bug report.",
  "settings.reportNoKeysTitle": "This report never contains your API keys",
  "settings.reportNoKeysBody":
    "It lists versions, paths, and configuration states only. Credentials are removed even if one somehow reached the text.",
  "settings.buildingReport": "Building the report...",
  "settings.noReport": "No report yet.",
  "settings.logsLocalHint": "Stored locally, redacted, and pruned after 7 days.",
  "settings.couldNotOpenLogFolder": "Could not open the log folder.",
  "settings.readingLogs": "Reading logs...",
  "settings.noLogEntries": "No log entries yet",
  "settings.noLogEntriesBody":
    "Eclyzer Bridge writes here when something happens. An empty log on a fresh install is normal.",
  "settings.lineCount": "{count} lines",
  "settings.newestAtBottom": "Newest entries are at the bottom.",
  "settings.couldNotSave": "Could not save that setting.",
  "settings.openDataFolder": "Open data folder",
  "settings.rememberFolder": "Remember the last project folder",
  "settings.logs": "Logs",
  "settings.logLevel": "Log level",
  "settings.diagnostics": "Diagnostics",
  "settings.independence":
    "Eclyzer Bridge is an independent open-source project and is not affiliated with, endorsed by, or sponsored by OpenAI or Anthropic.",

  "toast.providerUpdated": "Provider updated.",
  "toast.providerAdded": "Provider added.",
  "toast.keySaved": "The API key was saved securely.",
  "toast.providerDeleted": "Provider deleted.",
  "toast.providerCopied": "Copied {name}.",
  "toast.keyCopied": "The API key was copied to the new provider.",
  "toast.noKeyCopied": "This provider had no stored key to copy.",
  "toast.activated": "Activated {name} for {agent}.",
  "toast.undoDone": "The last change was undone.",
  "toast.resetDone": "{agent} reset to stock defaults.",
  "toast.resetRemoved": "Removed {count} settings.",
  "toast.resetRemovedOne": "Removed {count} setting.",
  "toast.logsCleared": "Logs cleared.",
  "toast.diagnosticsCopied": "Diagnostics copied.",
  "toast.testResultsCopied": "Test results copied.",

  "error.generic": "Something went wrong in Eclyzer Bridge.",
  "error.loadFailed": "This screen could not be loaded.",
  "error.noAgents": "No coding agent detected",
  "error.testResultsCopyFailed": "Could not copy the test results.",
  "error.copyDiagnosticsFailed": "Could not copy the diagnostics.",

  // ---- update banner ---------------------------------------------------
  "update.bannerTitle": "Eclyzer Bridge {version} is available",
  "update.bannerBody": "You are running {current}. Open Settings to review and install it.",
  "update.notNow": "Not now",
  "update.openSettings": "Open Settings to install",
} as const;

/** Every key the app may translate. */
export type MessageKey = keyof typeof en;

/** A complete dictionary for one language. */
export type Dictionary = Record<MessageKey, string>;

/**
 * Bahasa Indonesia.
 *
 * Kept short and phrased the way a developer would say it, rather than a literal
 * translation of the English. Technical nouns that are used untranslated in
 * Indonesian engineering practice - endpoint, provider, profile, API key, log -
 * are left as they are, because inventing Indonesian equivalents makes them
 * harder to read, not easier.
 */
const id: Dictionary = {
  "nav.overview": "Ringkasan",
  "nav.providers": "Provider",
  "nav.playground": "Playground",
  "nav.history": "Riwayat",
  "nav.settings": "Pengaturan",
  "app.tagline": "Jembatan konfigurasi lokal",
  "app.noTelemetry": "Tanpa telemetri",

  "action.cancel": "Batal",
  "action.delete": "Hapus",
  "action.edit": "Edit",
  "action.duplicate": "Duplikat",
  "action.test": "Tes",
  "action.activate": "Aktifkan",
  "action.active": "Aktif",
  "action.refresh": "Muat ulang",
  "action.retry": "Coba lagi",
  "action.browse": "Pilih...",
  "action.open": "Buka",
  "action.add": "Tambah",
  "action.remove": "Hapus",
  "action.done": "Selesai",
  "action.hide": "Sembunyikan",
  "action.copyDiagnostics": "Salin diagnostik",
  "action.openFolder": "Buka folder",
  "action.clearLogs": "Bersihkan log",

  "status.notTested": "Belum dites",
  "status.testing": "Sedang dites...",
  "status.compatible": "Kompatibel",
  "status.partiallyCompatible": "Kompatibel sebagian",
  "status.incompatible": "Tidak kompatibel",
  "status.notDetermined": "Belum ditentukan",
  "status.testFailed": "Tes gagal",
  "status.detected": "Terdeteksi",
  "status.notFound": "Tidak ditemukan",
  "status.configOk": "Config OK",
  "status.configUnreadable": "Config tidak terbaca",
  "status.noConfigYet": "Belum ada config",
  "status.externalConfiguration": "Konfigurasi eksternal",

  "home.title": "Ringkasan",
  "home.refresh": "Muat ulang",
  "home.noAgents": "Tidak ada coding agent terdeteksi",
  "home.installGuide": "Panduan instalasi",
  "home.configure": "Atur",
  "home.playground": "Playground",
  "home.history": "Riwayat",
  "home.version": "Versi",
  "home.versionValue": "Versi {version}",
  "home.versionUnknown": "tidak diketahui",
  "home.notInstalled": "Belum terinstall",
  "home.openAgent": "Buka {agent}",
  "home.checkingSetup": "Memeriksa setup kamu...",
  "home.intro":
    "Eclyzer Bridge mengarahkan Codex dan Claude Code ke provider pilihanmu, tanpa perlu mengedit file konfigurasi sendiri.",
  "home.setUpProvider": "Siapkan provider",
  "home.noAgentsLong":
    "Eclyzer Bridge tidak menemukan Codex atau Claude Code di komputer ini. Install salah satunya, lalu pilih Muat ulang.",
  "home.notFoundTitle": "{agent} tidak ditemukan",
  "home.notFoundBody":
    "Eclyzer Bridge tidak menginstall apa pun untukmu. Install {agent}, lalu pilih Muat ulang.",
  "home.invalidConfigTitle": "Eclyzer Bridge tidak akan mengubah konfigurasi ini",
  "home.invalidConfigBody":
    "{path} tidak valid. Eclyzer Bridge menolak menulis ulang file yang tidak bisa dibaca, karena itu akan menghapus setting yang tidak dikenalnya. Perbaiki filenya sendiri, lalu pilih Muat ulang.",
  "home.activeFallback": "Profil provider",
  "home.activeManagedHint": "diatur oleh Eclyzer Bridge",
  "home.externalHint": "setting milikmu sendiri; bukan ditulis Eclyzer Bridge",
  "home.defaultConfiguration": "Konfigurasi default",
  "home.defaultHint": "tidak ada provider custom yang diatur",
  "home.lastTested": "Terakhir dites",
  "home.configFile": "File config",
  "home.refreshAgent": "Muat ulang deteksi {agent}",
  "home.openLinkFailed": "Tidak bisa membuka tautan.",

  "launch.title": "Buka {agent}",
  "launch.folderLabel": "Folder proyek",
  "launch.folderHint":
    "{agent} dibuka di folder ini. Eclyzer Bridge mengirimnya sebagai satu argumen, jadi nama folder yang mengandung spasi atau tanda kutip tetap aman.",
  "launch.preview": "Lihat perintah",
  "launch.previewTitle": "Perintah yang akan dijalankan",
  "launch.previewHint":
    "Nama model dan provider dikirim sebagai argumen terpisah, tidak pernah dijalankan sebagai perintah shell.",
  "launch.body":
    "{agent} terbuka di jendela terminal baru pada folder yang kamu pilih. Eclyzer Bridge langsung keluar begitu jendela terbuka, jadi menutup Bridge tidak akan menutup agent kamu.",
  "launch.openedToast": "Terbuka di jendela terminal baru.",
  "launch.folderPickerFailed": "Tidak bisa membuka pemilih folder.",

  "providers.title": "Provider",
  "providers.subtitle":
    "Profil provider itu satu endpoint, satu model, dan kredensial. Eclyzer Bridge hanya menulis setting miliknya sendiri dan membiarkan sisanya apa adanya.",
  "providers.add": "Tambah provider",
  "providers.empty": "Belum ada provider",
  "providers.emptyBody": "Tambahkan provider untuk mengarahkan {agent} ke endpoint pilihanmu.",
  "providers.testFirst": "Tes provider ini ke agent tersebut dulu.",
  "providers.addKeyFirst": "Tambahkan API key sebelum mengaktifkan.",
  "providers.noKey": "Tanpa API key",
  "providers.model": "Model",
  "providers.targets": "Target",
  "providers.nothing": "tidak ada",
  "providers.tested": "dites {when}",
  "providers.loading": "Memuat provider kamu...",
  "providers.yourAgent": "agent kamu",
  "providers.noAgentsBody": "Install Codex atau Claude Code, lalu muat ulang halaman ini.",
  "providers.costNoticeTitle": "Tes memakai akun provider kamu",
  "providers.costNoticeBody":
    "Test Connection dan Playground mengirim request asli ke provider-mu dan bisa terhitung pemakaian atau biaya. Eclyzer Bridge tidak pernah mengirim apa pun sendiri.",
  "providers.costNoticeAck": "Mengerti",
  "providers.testAria": "Tes {name} ke {agent}",
  "providers.duplicateAria": "Duplikat {name}",
  "providers.deleteAria": "Hapus {name}",

  "editor.createTitle": "Tambah provider",
  "editor.baseUrl": "Base URL",
  "editor.apiKey": "API key",
  "editor.apiKeyKeep": "Biarkan kosong untuk mempertahankan key yang tersimpan.",
  "editor.apiKeyHint":
    "Disegel dengan Windows DPAPI untuk akunmu. Tidak pernah ditulis ke file profil, log, atau backup.",
  "editor.clearKey": "Hapus API key yang tersimpan",
  "editor.model": "Model ID",
  "editor.targets": "Dipakai untuk",
  "editor.notes": "Catatan",
  "editor.tabGeneral": "Umum",
  "editor.tabAdvanced": "Lanjutan",
  "editor.authMode": "Cara key dikirim",
  "editor.authModeBearer": "Authorization: Bearer (umumnya)",
  "editor.authModeXApiKey": "x-api-key (gaya Anthropic)",
  "editor.timeout": "Timeout request (ms)",
  "editor.headers": "Header tambahan",
  "editor.headersHint":
    "Nilainya disimpan apa adanya di profil dan ditulis ke file konfigurasi agent. Taruh kredensial gateway di kolom API key saja, karena di situ disegel dengan Windows DPAPI. Authorization, x-api-key, dan proxy-authorization tidak bisa ditimpa di sini.",
  "editor.headerName": "Nama header",
  "editor.headerValue": "Nilai",
  "editor.queryParams": "Query parameter",
  "editor.reset": "Reset opsi lanjutan",
  "editor.copyEndpoint": "Salin endpoint dan model",
  "editor.insecure": "Saya mengerti, kirim key saya lewat HTTP biasa",
  "editor.editTitleName": "Ubah {name}",
  "editor.saveChanges": "Simpan perubahan",
  "editor.tabsAria": "Pengaturan provider",
  "editor.presetLabel": "Mulai dari preset",
  "editor.presetHint":
    "Preset mengisi endpoint dan model yang sudah dikenal. Semuanya bisa kamu ubah setelahnya.",
  "editor.chooseProvider": "Pilih provider...",
  "editor.profileName": "Nama profil",
  "editor.profileNamePlaceholder": "DeepSeek untuk kerja",
  "editor.providerName": "Nama provider",
  "editor.providerNameHint":
    "Ditampilkan di konfigurasi agent supaya kamu bisa membedakan provider.",
  "editor.baseUrlHintApi":
    "Root API yang kompatibel dengan OpenAI atau Anthropic. Jangan sertakan /chat/completions.",
  "editor.modelHintExact":
    "Dikirim ke agent persis seperti yang ditulis. Tidak pernah dijalankan sebagai perintah.",
  "editor.keyStored":
    "API key sudah tersimpan ({fingerprint}). Biarkan kosong untuk mempertahankannya.",
  "editor.fingerprintStored": "tersimpan",
  "editor.noKeyStored": "Belum ada API key tersimpan. Isi satu untuk mengaktifkan provider ini.",
  "editor.clearKeyHint":
    "Provider-nya tetap ada, tapi tidak bisa diaktifkan sampai ada API key baru.",
  "editor.targetsHintProtocols":
    "Codex dan Claude Code memakai protokol yang berbeda. Masing-masing dites terpisah.",
  "editor.insecureTitle": "Endpoint tidak aman",
  "editor.insecureBody":
    "API key kamu bisa terkirim tanpa enkripsi. Endpoint di bawah memakai HTTP biasa dan bukan di mesin ini.",
  "editor.insecureHint":
    "Lakukan ini hanya untuk endpoint yang kamu kendalikan di jaringan terpercaya.",
  "editor.advancedBody":
    "Opsi ini untuk gateway yang butuh header tambahan atau gaya autentikasi lain. Umumnya provider tidak perlu satu pun.",
  "editor.authModeHint":
    "Bearer mengirim Authorization: Bearer <key>. x-api-key mengikuti konvensi Anthropic.",
  "editor.timeoutHint":
    "Milidetik. Gateway yang lambat mungkin butuh lebih lama; default-nya 20000.",
  "editor.noHeaders": "Tidak ada header tambahan.",
  "editor.noQueryParams": "Tidak ada query parameter tambahan.",
  "editor.valueHidden": "•••••• (nilai disembunyikan)",
  "editor.emptyValue": "(kosong)",
  "editor.removeHeaderAria": "Hapus header {name}",
  "editor.headerValueAria": "Nilai header",
  "editor.removeParamAria": "Hapus parameter {name}",
  "editor.modelAliasLabel": "Alias model",
  "editor.modelAliasNote":
    "Opsional. Ditulis ke agent sebagai nama model kalau provider mengharapkan ID yang berbeda dari yang kamu tes.",
  "editor.modelAliasPlaceholder": "sama dengan model ID",
  "editor.notesPlaceholder": "Apa pun yang ingin kamu ingat tentang provider ini.",
  "editor.noEndpoint": "Endpoint belum diatur",
  "editor.errorName": "Beri nama provider ini supaya kamu mudah mengenalinya.",
  "editor.errorProviderName": "Isi nama provider.",
  "editor.errorBaseUrl": "Isi URL endpoint.",
  "editor.errorBaseUrlScheme": "URL harus diawali http:// atau https://.",
  "editor.errorModel": "Isi model ID persis seperti yang diharapkan provider.",
  "editor.errorTargets": "Pilih minimal satu agent.",
  "editor.errorApiKey": "Isi API key untuk provider ini.",
  "editor.errorApiKeyShort": "Key-nya kelihatan terlalu pendek. Pastikan sudah tersalin semuanya.",
  "editor.errorInsecure":
    "Endpoint ini memakai HTTP biasa. Centang konfirmasi di bawah, atau pakai URL https://.",

  "delete.title": "Hapus {name}?",
  "delete.body": "Profil dan API key yang tersimpan akan dihapus. Konfigurasi agent tidak diubah.",
  "delete.anyway": "Tetap hapus",
  "delete.confirm": "Hapus provider",
  "delete.impactFailed": "Tidak bisa memeriksa apakah provider ini sedang aktif.",
  "delete.activeTitle": "Provider ini sedang aktif",
  "delete.activeBody":
    "{agent} sedang memakai {name}. Kalau dihapus sekarang, agent itu akan menunjuk ke konfigurasi yang tidak lagi dikelola Eclyzer Bridge.",
  "delete.anAgent": "Satu agent",
  "delete.agentJoin": " dan ",
  "delete.saferOrder":
    "Urutan yang lebih aman: aktifkan provider lain, atau kembalikan konfigurasi default, baru hapus yang ini.",
  "delete.switchFirstTitle": "Pilih provider lain atau kembalikan konfigurasi default dulu.",
  "delete.keepAndSwitch": "Biarkan - saya ganti dulu",

  "test.title": "Hasil tes",
  "test.passed": "Lolos",
  "test.failed": "Gagal",
  "test.warning": "Perhatian",
  "test.skipped": "Dilewati",
  "test.copyResults": "Salin hasil",
  "test.summary": "{passed} dari {total} pemeriksaan lolos",
  "test.reason": "Alasan",
  "test.totalTime": "Total waktu",

  "history.title": "Riwayat",
  "history.subtitle":
    "Setiap perubahan Eclyzer Bridge bisa dibatalkan, dan setiap tombol di sini menjelaskan apa yang akan dilakukan sebelum melakukannya.",
  "history.tabChanges": "Perubahan",
  "history.tabSnapshots": "Snapshot",
  "history.tabRecovery": "Pulihkan",
  "history.viewLabel": "Tampilan riwayat",
  "history.noAgentsBody":
    "Riwayat muncul di sini setelah Eclyzer Bridge bisa mendeteksi Codex atau Claude Code.",
  "history.empty": "Belum ada perubahan",
  "history.emptyBody":
    "Perubahan muncul di sini setelah Eclyzer Bridge mengubah konfigurasi agent.",
  "history.loadingChanges": "Memuat riwayat perubahan...",
  "history.tableCaption": "Perubahan yang dibuat Eclyzer Bridge pada agent ini",
  "history.when": "Waktu",
  "history.what": "Perubahan",
  "history.result": "Hasil",
  "history.undoable": "Bisa dibatalkan",
  "history.succeeded": "Berhasil",
  "history.failed": "Gagal",
  "history.yes": "Ya",
  "history.restore": "Pulihkan",
  "history.loadingSnapshots": "Memuat snapshot...",
  "history.noSnapshots": "Belum ada snapshot",
  "history.noSnapshotsBody":
    "Snapshot dibuat sebelum setiap perubahan, jadi selalu ada yang bisa dikembalikan. Untuk agent ini belum ada.",
  "history.newestKept": "20 snapshot terbaru disimpan",
  "history.newestKeptBody":
    "Yang lebih lama dihapus otomatis. Konfigurasi aslimu disimpan terpisah dan tidak pernah dihapus.",
  "history.undoTitle": "Batalkan perubahan terakhir",
  "history.undoBody": "Mengembalikan agent ini ke kondisi tepat sebelum perubahan terakhirnya.",
  "history.undoNothing": " Tidak ada yang bisa dibatalkan.",
  "history.undoReversible":
    " Kondisi sekarang disimpan dulu, jadi pembatalan ini pun bisa dibatalkan.",
  "history.undoButton": "Batalkan perubahan terakhir",
  "history.restoreDefaultTitle": "Kembalikan yang diubah Eclyzer Bridge",
  "history.restoreDefaultBody":
    "Menghapus provider yang ditambahkan Eclyzer Bridge dan mengembalikan agent ini ke default-nya. Sisanya tidak disentuh. Setting yang kamu buat sendiri tetap dipertahankan, meskipun terlihat seperti buatan Eclyzer Bridge.",
  "history.restoreDefaultButton": "Pulihkan bawaan",
  "history.restoreOriginalTitle": "Pulihkan konfigurasi awal",
  "history.restoreOriginalBody":
    "Mengembalikan file persis seperti sebelum Eclyzer Bridge menyentuh agent ini. Ini pemulihan historis, bukan reset pabrik: kalau file itu sudah berisi URL gateway, model, atau alias milikmu, nilai itu ikut kembali.",
  "history.restoreOriginalUnavailable":
    "Tersedia setelah Eclyzer Bridge mengubah agent ini minimal sekali.",
  "history.restoreOriginalButton": "Pulihkan konfigurasi awal",
  "history.checkingRecovery": "Memeriksa opsi pemulihan...",
  "history.recoveryUnreadable": "Opsi pemulihan tidak bisa dibaca.",
  "history.currently": "Saat ini",
  "history.currentManaged": "Konfigurasi Eclyzer Bridge sedang aktif",
  "history.originalBackup": "Backup asli",
  "history.lastChange": "Perubahan terakhir",
  "history.externalTitle": "Konfigurasimu tidak ditulis Eclyzer Bridge",
  "history.externalBody":
    "Agent ini memakai konfigurasi custom yang bukan dibuat Eclyzer Bridge. Mengembalikan default akan menggantinya, jadi opsi di bawah ini ditawarkan dengan pertimbangan itu.",
  "history.resetTitle": "Reset {agent} ke setelan bawaan",
  "history.resetBody":
    "Menghapus semua override provider dan model, termasuk yang kamu atur sendiri sebelum memakai Eclyzer Bridge, supaya {agent} kembali ke konfigurasi bawaannya. Login, sesi, konfigurasi MCP, permission, dan setting lain tetap dipertahankan.",
  "history.resetButton": "Reset ke setelan bawaan...",
  "history.resetConfirmTitle": "Reset {agent} ke setelan bawaan?",
  "history.resetConfirmBody":
    "Ini menghapus override model dan provider, termasuk setting yang bukan dibuat Eclyzer Bridge. Login, sesi, konfigurasi MCP, permission, dan setting lain akan dipertahankan.",
  "history.resetWillRemove": "Setting berikut akan dihapus:",
  "history.resetNothing":
    "Tidak ada override provider atau model, jadi tidak ada yang berubah. Tetap bisa dijalankan kalau mau memastikan.",
  "history.resetSnapshot":
    "Snapshot dibuat dulu, jadi ini bisa dibatalkan atau dikembalikan ke konfigurasi asli setelahnya.",
  "history.resetAction": "Reset {agent}",
  "history.resetGroupProvider": "Provider dan kredensial",
  "history.resetGroupAliases": "Alias model",
  "history.resetGroupModel": "Pemilihan model",
  "history.managedWarning":
    "Setting ini dikelola organisasimu di {paths}. Eclyzer Bridge tidak akan mengubahnya, jadi reset mungkin tidak berpengaruh. Tanyakan ke administrator.",
  "history.originalSaved": "Tersimpan: kondisi sebelum Eclyzer Bridge mengubah apa pun.",
  "history.originalMissing": "Belum tersimpan.",
  "history.confirmRestoreSnapshot": "Pulihkan snapshot dari {when}?",
  "history.confirmRestoreSnapshotBody":
    'Konfigurasi {agent} kamu sekarang diganti dengan kondisi yang disimpan sebelum "{label}".',
  "history.confirmRestoreDefault": "Kembalikan konfigurasi default?",
  "history.confirmRestoreDefaultBody":
    "Provider yang ditambahkan Eclyzer Bridge dihapus. Setting lain tetap seperti sekarang.",
  "history.confirmRestoreOriginal": "Kembalikan konfigurasi asli?",
  "history.confirmRestoreOriginalBody":
    "File konfigurasimu diganti dengan salinan sebelum Eclyzer Bridge mengubahnya. Perubahan yang kamu buat setelah itu akan hilang.",
  "history.restored": "Konfigurasi dipulihkan.",
  "history.nowActive": "Aktif sekarang",
  "history.snapshotTaken": "Snapshot dibuat",
  "history.keysUpdated": "Key diperbarui",
  "history.snapshotFirstTitle": "Snapshot dibuat lebih dulu",
  "history.snapshotFirstBody": "Kondisi yang ada sekarang disimpan, jadi ini bisa dibatalkan.",
  "history.reversibleNote":
    "Kondisi yang baru kamu ganti sudah disimpan sebagai snapshot, jadi masih bisa dikembalikan dari daftar Snapshot.",
  "history.restoredBody": "Konfigurasi {agent} sudah ditulis dan diverifikasi.",

  "playground.title": "Playground",
  "playground.subtitle":
    "Kirim satu prompt ke provider dan lihat balasan mentahnya. Tidak ada yang disimpan.",
  "playground.provider": "Provider",
  "playground.model": "Model",
  "playground.endpoint": "Endpoint",
  "playground.prompt": "Prompt",
  "playground.send": "Kirim",
  "playground.sending": "Mengirim...",
  "playground.agentTabsAria": "Coding agent",
  "playground.billableTitle": "Request ini berbayar",
  "playground.billableBody":
    "Prompt di bawah dikirim ke provider-mu dan bisa terhitung pemakaian atau biaya. Tidak ada yang dikirim kalau kamu tidak menekan Kirim.",
  "playground.noUsableProvider": "Tidak ada provider yang bisa dipakai untuk agent ini",
  "playground.noUsableProviderBody":
    "Tambahkan provider untuk {agent} dan isi API key, lalu kembali ke sini.",
  "playground.formTitle": "Kirim prompt",
  "playground.promptHint":
    "Hanya satu putaran. Balasan provider ditampilkan persis seperti yang diterima.",
  "playground.askShort": "Balas dengan satu kata: siap",
  "playground.waiting": "Menunggu provider...",
  "playground.requestFailed": "Request gagal.",
  "playground.responseReceived": "Balasan diterima",
  "playground.streamed": "Streaming",
  "playground.transport": "Transport",
  "playground.technicalDetail": "Detail teknis",
  "playground.tokensInline": "{input} masuk / {output} keluar token",

  "settings.title": "Pengaturan",
  "settings.subtitle":
    "Preferensi lokal. Tidak ada yang dikirim ke mana pun, dan tidak ada yang menghubungi server.",
  "settings.language": "Bahasa",
  "settings.languageHint": "Langsung berlaku dan diingat.",
  "settings.updates": "Update",
  "settings.updateCheck": "Periksa update otomatis",
  "settings.checkNow": "Periksa sekarang",
  "settings.checking": "Memeriksa...",
  "settings.updateInstall": "Unduh dan pasang",
  "settings.openProjectPage": "Halaman proyek",
  "settings.generalSection": "Umum",
  "settings.aboutSection": "Tentang",
  "settings.updateCheckOn": "Eclyzer Bridge akan memeriksa update dari GitHub.",
  "settings.updateCheckOff": "Pemeriksaan update dimatikan.",
  "settings.updateCheckBody":
    "Maksimal sekali per 24 jam, dari GitHub Releases. Tidak ada informasi tentangmu atau mesinmu yang dikirim; ini cuma request unduhan biasa. Update tidak pernah dipasang tanpa konfirmasimu.",
  "settings.rememberFolderBody":
    "Dipakai untuk menyarankan folder saat kamu membuka coding agent. Kalau dimatikan, lokasi yang tersimpan langsung dilupakan.",
  "settings.logLevelBody":
    "Log hanya tersimpan di mesin ini dan dihapus setelah 7 hari. Level lebih tinggi berguna saat melaporkan bug.",
  "settings.logLevelError": "Error - hanya masalah",
  "settings.logLevelWarn": "Warn",
  "settings.logLevelInfo": "Info - disarankan",
  "settings.logLevelDebug": "Debug",
  "settings.logLevelTrace": "Trace - sangat detail",
  "settings.logLevelSaved": "Level log disimpan.",
  "settings.protectedBy": "Dilindungi oleh",
  "settings.storedApiKeys": "API key tersimpan",
  "settings.none": "tidak ada",
  "settings.providerProfiles": "Profil provider",
  "settings.credentialHelper": "Credential helper",
  "settings.credentialHelperAvailable":
    "Tersedia - Codex membaca key tanpa perlu ditulis ke config",
  "settings.credentialHelperNotFound": "Tidak ditemukan - Codex memakai environment variable",
  "settings.dataFolder": "Folder data",
  "settings.couldNotOpenDataFolder": "Folder data tidak bisa dibuka.",
  "settings.couldNotOpenBrowser": "Browser tidak bisa dibuka.",
  "settings.lastChecked": "Terakhir diperiksa {when}",
  "settings.notCheckedYet": "Belum pernah diperiksa",
  "settings.updateCheckIncomplete": "Pemeriksaan update tidak selesai",
  "settings.upToDateTitle": "Kamu sudah versi terbaru",
  "settings.latestRelease": "Eclyzer Bridge {version} adalah rilis terbaru.",
  "settings.updateAvailableTitle": "Versi {version} tersedia",
  "settings.updateBody":
    "Kamu memakai {current}. Update diunduh dari GitHub Releases dan diverifikasi tanda tangannya sebelum ada yang diganti.",
  "settings.releaseNotes": "Catatan rilis",
  "settings.restartOnInstall": "Eclyzer Bridge restart setelah instalasi selesai.",
  "settings.diagnosticsBody": "Ringkasan setup-mu yang bisa ditempel ke laporan bug.",
  "settings.reportNoKeysTitle": "Laporan ini tidak pernah memuat API key-mu",
  "settings.reportNoKeysBody":
    "Isinya hanya versi, lokasi, dan status konfigurasi. Kredensial dihapus walaupun entah bagaimana sampai ke teksnya.",
  "settings.buildingReport": "Menyusun laporan...",
  "settings.noReport": "Belum ada laporan.",
  "settings.logsLocalHint": "Tersimpan lokal, disamarkan, dan dihapus setelah 7 hari.",
  "settings.couldNotOpenLogFolder": "Folder log tidak bisa dibuka.",
  "settings.readingLogs": "Membaca log...",
  "settings.noLogEntries": "Belum ada entri log",
  "settings.noLogEntriesBody":
    "Eclyzer Bridge menulis ke sini kalau ada yang terjadi. Log kosong di instalasi baru itu normal.",
  "settings.lineCount": "{count} baris",
  "settings.newestAtBottom": "Entri terbaru ada di bawah.",
  "settings.couldNotSave": "Setting itu tidak bisa disimpan.",
  "settings.openDataFolder": "Buka folder data",
  "settings.rememberFolder": "Ingat folder proyek terakhir",
  "settings.logs": "Log",
  "settings.logLevel": "Level log",
  "settings.diagnostics": "Diagnostik",
  "settings.independence":
    "Eclyzer Bridge adalah proyek open-source independen dan tidak berafiliasi dengan, tidak didukung oleh, dan tidak disponsori oleh OpenAI atau Anthropic.",

  "toast.providerUpdated": "Provider diperbarui.",
  "toast.providerAdded": "Provider ditambahkan.",
  "toast.keySaved": "API key tersimpan dengan aman.",
  "toast.providerDeleted": "Provider dihapus.",
  "toast.providerCopied": "{name} disalin.",
  "toast.keyCopied": "API key ikut disalin ke provider baru.",
  "toast.noKeyCopied": "Provider ini tidak punya key tersimpan untuk disalin.",
  "toast.activated": "{name} diaktifkan untuk {agent}.",
  "toast.undoDone": "Perubahan terakhir dibatalkan.",
  "toast.resetDone": "{agent} direset ke setelan bawaan.",
  "toast.resetRemoved": "{count} setting dihapus.",
  "toast.resetRemovedOne": "{count} setting dihapus.",
  "toast.logsCleared": "Log dibersihkan.",
  "toast.diagnosticsCopied": "Diagnostik disalin.",
  "toast.testResultsCopied": "Hasil tes disalin.",

  "error.generic": "Ada yang tidak beres di Eclyzer Bridge.",
  "error.loadFailed": "Layar ini gagal dimuat.",
  "error.noAgents": "Tidak ada coding agent terdeteksi",
  "error.testResultsCopyFailed": "Tidak bisa menyalin hasil tes.",
  "error.copyDiagnosticsFailed": "Tidak bisa menyalin diagnostik.",

  "update.bannerTitle": "Eclyzer Bridge {version} tersedia",
  "update.bannerBody": "Kamu memakai {current}. Buka Pengaturan untuk meninjau dan memasangnya.",
  "update.notNow": "Nanti saja",
  "update.openSettings": "Buka Pengaturan untuk memasang",
};

const DICTIONARIES: Record<Language, Dictionary> = { en, id };

/**
 * The dictionary for a language, falling back to English for any key it lacks.
 *
 * The fallback exists so a partially translated language still renders rather
 * than showing blank space, but the type of `Dictionary` means a *missing* key is
 * caught at build time; the fallback only matters if a translation is added at
 * runtime.
 */
export function dictionaryFor(language: Language): Dictionary {
  return DICTIONARIES[language] ?? DICTIONARIES.en;
}

/** Values substituted into `{placeholders}`. */
export type Interpolations = Record<string, string | number>;

/**
 * Look up `key` and substitute its placeholders.
 *
 * A missing placeholder is left as-is rather than replaced with "undefined",
 * so a formatting mistake is visible in the UI instead of silently producing a
 * sentence that reads wrongly.
 */
export function translate(
  dictionary: Dictionary,
  key: MessageKey,
  values?: Interpolations,
): string {
  const template = dictionary[key] ?? en[key] ?? key;
  if (!values) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in values ? String(values[name]) : match,
  );
}

/**
 * Translate backend error copy.
 *
 * The backend sends English messages, and translating them would require the two
 * sides to agree on a key set. Rather than pretend, the known shapes are matched
 * and everything else falls back to the backend's own wording, which is already
 * written for a developer to read.
 */
export function translateError(
  dictionary: Dictionary,
  error: ApiError,
): { message: string; hint?: string } {
  const message = error.message?.trim() ? error.message : translate(dictionary, "error.generic");
  return { message, hint: error.hint };
}
