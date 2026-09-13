import { describe, expect, it } from "vitest";

import type { BrowserTabs } from "../../src/core/infrastructure/browser/BrowserAdapter";
import { RemotePortalAdapter } from "../../src/core/infrastructure/portal/RemotePortalAdapter";

interface SentMessage {
  readonly tabId: number;
  readonly command: unknown;
}

/**
 * Tab stub that records every target it was asked to message, so tests can
 * assert *which* tab a command reached rather than only that it was sent.
 */
class FakeTabs implements BrowserTabs {
  readonly sent: SentMessage[] = [];

  /** Successive values returned by getActiveTabId, mimicking focus changes. */
  private readonly activeTabIds: (number | undefined)[];
  private reply: unknown = { success: true, data: true };

  constructor(activeTabIds: (number | undefined)[]) {
    this.activeTabIds = [...activeTabIds];
  }

  getActiveTabId(): Promise<number | undefined> {
    // Hold the final value once the script has run out, so a batch that issues
    // more commands than there are focus changes still resolves something.
    const next =
      this.activeTabIds.length > 1
        ? this.activeTabIds.shift()
        : this.activeTabIds[0];

    return Promise.resolve(next);
  }

  sendMessage(tabId: number, command: unknown): Promise<unknown> {
    this.sent.push({ tabId, command });

    return Promise.resolve(this.reply);
  }

  respondWith(reply: unknown): void {
    this.reply = reply;
  }

  get targetedTabIds(): number[] {
    return this.sent.map((entry) => entry.tabId);
  }
}

describe("RemotePortalAdapter", () => {
  describe("attach", () => {
    it("binds to the active tab", async () => {
      const tabs = new FakeTabs([7]);
      const portal = new RemotePortalAdapter(tabs);

      const result = await portal.attach();

      expect(result).toEqual({ success: true, data: 7 });
      expect(portal.attachedTabId).toBe(7);
    });

    it("fails when no tab is active", async () => {
      const portal = new RemotePortalAdapter(new FakeTabs([undefined]));

      const result = await portal.attach();

      expect(result.success).toBe(false);
      expect(portal.attachedTabId).toBeNull();
    });
  });

  describe("tab targeting", () => {
    it("keeps using the attached tab after focus moves elsewhere", async () => {
      // Tab 1 is active at attach time; focus then moves to tab 2 and tab 3.
      const tabs = new FakeTabs([1, 2, 3]);
      const portal = new RemotePortalAdapter(tabs);

      await portal.attach();
      await portal.getTrainees();
      await portal.openTrainingForm();
      await portal.submitTrainingForm();

      // Every command must land on tab 1. If any reached 2 or 3, a submission
      // would be firing against a page this batch never filled in.
      expect(tabs.targetedTabIds).toEqual([1, 1, 1]);
    });

    it("refuses to dispatch before attach", async () => {
      const tabs = new FakeTabs([1]);
      const portal = new RemotePortalAdapter(tabs);

      const result = await portal.submitTrainingForm();

      expect(result.success).toBe(false);
      // Unattached must mean "send nothing", not "guess at the active tab".
      expect(tabs.sent).toHaveLength(0);
    });

    it("refuses to dispatch after detach", async () => {
      const tabs = new FakeTabs([1]);
      const portal = new RemotePortalAdapter(tabs);

      await portal.attach();
      portal.detach();

      const result = await portal.submitTrainingForm();

      expect(result.success).toBe(false);
      expect(tabs.sent).toHaveLength(0);
    });

    it("rebinds to the newly active tab on re-attach", async () => {
      const tabs = new FakeTabs([1, 9]);
      const portal = new RemotePortalAdapter(tabs);

      await portal.attach();
      portal.detach();
      await portal.attach();
      await portal.getTrainees();

      expect(portal.attachedTabId).toBe(9);
      expect(tabs.targetedTabIds).toEqual([9]);
    });
  });

  describe("portal page state", () => {
    it("checks the page live on every call", async () => {
      const tabs = new FakeTabs([1]);
      const portal = new RemotePortalAdapter(tabs);
      await portal.attach();

      expect(await portal.isPortalPage()).toBe(true);

      // The session lapses mid-batch. A cached answer would keep reporting the
      // page as usable and let the run continue submitting into a dead session.
      tabs.respondWith({ success: true, data: false });

      expect(await portal.isPortalPage()).toBe(false);
    });

    it("reports false when unattached", async () => {
      const tabs = new FakeTabs([1]);
      const portal = new RemotePortalAdapter(tabs);

      // Optimistically defaulting to true here would vouch for a page that has
      // never been checked at all.
      expect(await portal.isPortalPage()).toBe(false);
      expect(tabs.sent).toHaveLength(0);
    });

    it("reports false when the content script is unreachable", async () => {
      const tabs = new FakeTabs([1]);
      tabs.respondWith({ success: false, error: "no receiving end" });

      const portal = new RemotePortalAdapter(tabs);
      await portal.attach();

      expect(await portal.isPortalPage()).toBe(false);
    });
  });
});
