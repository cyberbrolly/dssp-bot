import type {
  Message,
  MessageResponse,
} from "./Messages";

export class MessageBus {
  async send(
    message: Message,
  ): Promise<MessageResponse> {
    try {
      return await chrome.runtime.sendMessage(message);
    } catch (error) {
      return {
        success: false,
        error:
          error instanceof Error
            ? error.message
            : String(error),
      };
    }
  }

  listen(
    handler: (
      message: Message,
      sender: chrome.runtime.MessageSender,
    ) => Promise<MessageResponse> | MessageResponse,
  ): void {
    chrome.runtime.onMessage.addListener(
      (message, sender, sendResponse) => {
        Promise.resolve()
          .then(() =>
            handler(message as Message, sender),
          )
          .then(sendResponse)
          .catch((error: unknown) => {
            sendResponse({
              success: false,
              error:
                error instanceof Error
                  ? error.message
                  : String(error),
            });
          });

        return true;
      },
    );
  }
}