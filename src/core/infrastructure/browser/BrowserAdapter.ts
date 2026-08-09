export interface BrowserStorageArea {
  get(key: string): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove(key: string): Promise<void>;
  clear(): Promise<void>;
}

export interface BrowserMessageSender {
  tabId?: number;
  url?: string;
}

export type BrowserMessageListener = (
  message: unknown,
  sender: BrowserMessageSender,
) => Promise<unknown>;

export interface BrowserRuntime {
  sendMessage(message: unknown): Promise<unknown>;
  onMessage(listener: BrowserMessageListener): void;
}

export interface BrowserTabs {
  getActiveTabId(): Promise<number | undefined>;
  sendMessage(tabId: number, message: unknown): Promise<unknown>;
}

export interface BrowserAdapter {
  readonly storage: BrowserStorageArea;
  readonly runtime: BrowserRuntime;
  readonly tabs: BrowserTabs;
}
