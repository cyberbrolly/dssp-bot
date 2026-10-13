import {
  DAEMON_SETTINGS_KEY,
  proxyDaemonMessage,
} from "../core/infrastructure/daemon/DaemonClient";
import type { DaemonSettings } from "../core/infrastructure/daemon/DaemonClient";
import type { Message } from "../core/infrastructure/messaging/Messages";

// Stateless compatibility bridge only. Rust owns execution, reports and recovery.
// Portal tabs/content scripts are not admitted as callers of the authenticated API.
chrome.runtime.onMessage.addListener(
  (message: Message, sender, sendResponse) => {
    if (
      sender.id !== chrome.runtime.id ||
      sender.tab ||
      !sender.url?.startsWith(chrome.runtime.getURL(""))
    ) {
      sendResponse({
        success: false,
        error: "Only extension UI requests are allowed.",
      });
      return false;
    }
    void proxyDaemonMessage(message, async () => {
      const stored = await chrome.storage.local.get(DAEMON_SETTINGS_KEY);
      const settings = stored[DAEMON_SETTINGS_KEY] as
        DaemonSettings | undefined;
      if (!settings)
        throw new Error(
          "Configure the daemon port and paste its token manually in the popup.",
        );
      return settings;
    }).then(sendResponse);
    return true;
  },
);
