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
  async findDsspTraineeTab(): Promise<{ id?: number; url?: string; status?: string } | undefined> {
    const tabs = await chrome.tabs.query({ url: "https://dssp.frsc.gov.ng/Trainee*" });
    const tab = tabs.find((candidate) => candidate.active) ?? tabs[0];
    return tab ? { id: tab.id, url: tab.url, status: tab.status } : undefined;
  }

  async getActiveTab(): Promise<{ id?: number; url?: string; status?: string } | undefined> {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    return tab ? { id: tab.id, url: tab.url, status: tab.status } : undefined;
  }

  async getActiveTabId(): Promise<number | undefined> {
    return (await this.getActiveTab())?.id;
  }

  sendMessage(tabId: number, message: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      chrome.tabs.sendMessage(tabId, message, (response) => {
        const runtimeError = chrome.runtime.lastError;
        if (runtimeError) {
          reject(new Error(runtimeError.message ?? "Content script did not respond"));
          return;
        }
        resolve(response);
      });
    });
  }
}

export class ChromiumBrowserAdapter implements BrowserAdapter {
  readonly storage = new ChromiumStorageArea();
  readonly runtime = new ChromiumRuntime();
  readonly tabs = new ChromiumTabs();
}
