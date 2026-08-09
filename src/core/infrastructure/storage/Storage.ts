import type { BrowserStorageArea } from "../browser/BrowserAdapter";

export class Storage {
  private readonly area: BrowserStorageArea;

  constructor(area: BrowserStorageArea) {
    this.area = area;
  }

  async get<T>(key: string): Promise<T | undefined> {
    const result = await this.area.get(key);

    return result[key] as T | undefined;
  }

  async set<T>(key: string, value: T): Promise<void> {
    await this.area.set({
      [key]: value,
    });
  }

  async remove(key: string): Promise<void> {
    await this.area.remove(key);
  }

  async clear(): Promise<void> {
    await this.area.clear();
  }
}
