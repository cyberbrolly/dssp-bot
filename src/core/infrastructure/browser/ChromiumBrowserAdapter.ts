import type {
  BrowserAdapter,
  BrowserMessageListener,
  BrowserRuntime,
  BrowserStorageArea,
  BrowserTabs,
} from "./BrowserAdapter";

class ChromiumStorageArea implements BrowserStorageArea {
  get(key: string): Promise<Record<string, unknown>> {
    return chrome.storage.local.get(key);
  }

  set(items: Record<string, unknown>): Promise<void> {
    return chrome.storage.local.set(items);
  }

  remove(key: string): Promise<void> {
    return chrome.storage.local.remove(key);
  }

  clear(): Promise<void> {
    return chrome.storage.local.clear();
  }
}

class ChromiumRuntime implements BrowserRuntime {
  sendMessage(message: unknown): Promise<unknown> {
    return chrome.runtime.sendMessage(message);
  }

  onMessage(listener: BrowserMessageListener): void {
    chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
      listener(message, {
        ...(sender.tab?.id === undefined ? {} : { tabId: sender.tab.id }),
        ...(sender.url === undefined ? {} : { url: sender.url }),
      })
        .then(sendResponse)
        .catch((error: unknown) => {
          sendResponse({
            success: false,
            error: error instanceof Error ? error.message : String(error),
          });
        });

      return true;
    });
  }
}

class ChromiumTabs implements BrowserTabs {
  async getActiveTabId(): Promise<number | undefined> {
    const [tab] = await chrome.tabs.query({
      active: true,
      currentWindow: true,
    });

    return tab?.id;
  }

  sendMessage(tabId: number, message: unknown): Promise<unknown> {
    return chrome.tabs.sendMessage(tabId, message);
  }
}

export class ChromiumBrowserAdapter implements BrowserAdapter {
  readonly storage = new ChromiumStorageArea();
  readonly runtime = new ChromiumRuntime();
  readonly tabs = new ChromiumTabs();
}
