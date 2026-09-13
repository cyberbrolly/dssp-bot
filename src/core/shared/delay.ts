export function delay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(new Error("Delay aborted."));

      return;
    }

    const onAbort = (): void => {
      clearTimeout(timeoutId);
      reject(new Error("Delay aborted."));
    };

    const timeoutId = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);

    signal?.addEventListener("abort", onAbort, {
      once: true,
    });
  });
}
