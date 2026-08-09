export class DOMObserver {
  waitForElement<T extends Element>(
    selector: string,
    timeout = 10_000,
  ): Promise<T> {
    const existing = document.querySelector<T>(selector);

    if (existing) {
      return Promise.resolve(existing);
    }

    return new Promise((resolve, reject) => {
      const observer = new MutationObserver(() => {
        const element = document.querySelector<T>(selector);

        if (!element) {
          return;
        }

        observer.disconnect();
        clearTimeout(timeoutId);

        resolve(element);
      });

      observer.observe(document.body, {
        childList: true,
        subtree: true,
      });

      const timeoutId = window.setTimeout(() => {
        observer.disconnect();

        reject(
          new Error(
            `Timed out waiting for element: ${selector}`,
          ),
        );
      }, timeout);
    });
  }
}