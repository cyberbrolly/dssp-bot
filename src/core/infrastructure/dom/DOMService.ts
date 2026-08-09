export class DOMService {
  find<T extends Element>(
    selector: string,
    root: ParentNode = document,
  ): T | null {
    return root.querySelector<T>(selector);
  }

  findRequired<T extends Element>(
    selector: string,
    root: ParentNode = document,
  ): T {
    const element = this.find<T>(selector, root);

    if (!element) {
      throw new Error(`Required element not found: ${selector}`);
    }

    return element;
  }

  click(
    selector: string,
    root: ParentNode = document,
  ): void {
    const element = this.findRequired<HTMLElement>(selector, root);

    element.click();
  }

  setValue(
    selector: string,
    value: string,
    root: ParentNode = document,
  ): void {
    const element = this.findRequired<
      HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement
    >(selector, root);

    element.value = value;

    element.dispatchEvent(
      new Event("input", { bubbles: true }),
    );

    element.dispatchEvent(
      new Event("change", { bubbles: true }),
    );
  }

  exists(
    selector: string,
    root: ParentNode = document,
  ): boolean {
    return this.find(selector, root) !== null;
  }
}