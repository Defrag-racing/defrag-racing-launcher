; Installer wording. Replaces the strings Tauri's NSIS template ships with,
; which describe the two paths as "Do not uninstall" and "Uninstall before
; installing" - neither of which contains the word people are looking for when
; they double-click a newer build, and neither of which says what happens to
; their settings.
;
; Every string the template uses has to be here: this file REPLACES the default
; one rather than merging with it, and a missing LangString is a build error.
;
; ${VERSION} is the version being installed, ${PRODUCTNAME} the app name, and
; $R4 (in olderOrUnknownVersionInstalled only) is "older" or "unknown".
; {{product_name}} is substituted by the bundler, not by NSIS - leave it.

LangString addOrReinstall ${LANG_ENGLISH} "Repair this installation"
LangString alreadyInstalled ${LANG_ENGLISH} "Already installed"
LangString alreadyInstalledLong ${LANG_ENGLISH} "${PRODUCTNAME} ${VERSION} is already installed. Repair it if something is missing or broken, or remove it. Your settings, your login and your demos are not touched either way."
LangString appRunning ${LANG_ENGLISH} "{{product_name}} is running! Please close it first then try again."
LangString appRunningOkKill ${LANG_ENGLISH} "{{product_name}} is running!$\nClick OK to close it."
LangString chooseMaintenanceOption ${LANG_ENGLISH} "Repair or remove this installation."
LangString choowHowToInstall ${LANG_ENGLISH} "Choose how to install ${PRODUCTNAME} ${VERSION}."
LangString createDesktop ${LANG_ENGLISH} "Create a desktop shortcut"
LangString dontUninstall ${LANG_ENGLISH} "Update to ${VERSION}"
LangString dontUninstallDowngrade ${LANG_ENGLISH} "Install over the newer version (not possible - remove it first)"
LangString failedToKillApp ${LANG_ENGLISH} "Could not close {{product_name}}. Please close it yourself and try again."
LangString installingWebview2 ${LANG_ENGLISH} "Installing WebView2..."
LangString newerVersionInstalled ${LANG_ENGLISH} "A newer version of ${PRODUCTNAME} is installed. To go back to ${VERSION}, the newer version has to be removed first - your settings, your login and your demos stay where they are. Choose what to do and click Next."
LangString older ${LANG_ENGLISH} "older"
LangString olderOrUnknownVersionInstalled ${LANG_ENGLISH} "An $R4 version of ${PRODUCTNAME} is installed. Updating replaces it in place and keeps your settings, your login and your demos folder; removing it first installs a fresh copy and keeps them too. Choose what to do and click Next."
LangString silentDowngrades ${LANG_ENGLISH} "A newer version is installed. Remove it first, then run this installer again.$\n"
LangString unableToUninstall ${LANG_ENGLISH} "Could not remove the installed version."
LangString uninstallApp ${LANG_ENGLISH} "Uninstall ${PRODUCTNAME}"
LangString uninstallBeforeInstalling ${LANG_ENGLISH} "Remove the installed version first, then install ${VERSION}"
LangString unknown ${LANG_ENGLISH} "unknown"
LangString webview2AbortError ${LANG_ENGLISH} "Failed to install WebView2! The app can't run without it. Try restarting the installer."
LangString webview2DownloadError ${LANG_ENGLISH} "Error: Downloading WebView2 Failed - $0"
LangString webview2DownloadSuccess ${LANG_ENGLISH} "WebView2 bootstrapper downloaded successfully"
LangString webview2Downloading ${LANG_ENGLISH} "Downloading WebView2 bootstrapper..."
LangString webview2InstallError ${LANG_ENGLISH} "Error: Installing WebView2 failed with exit code $1"
LangString webview2InstallSuccess ${LANG_ENGLISH} "WebView2 installed successfully"
LangString deleteAppData ${LANG_ENGLISH} "Also delete settings, login and backup records (your demo files are never touched)"
