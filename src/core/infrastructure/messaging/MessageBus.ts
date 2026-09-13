import type {
  BrowserMessageSender,
  BrowserRuntime,
} from "../browser/BrowserAdapter";
import type { Message, MessageResponse } from "./Messages";

export type MessageHandler = (
  message: Message,
  sender: BrowserMessageSender,
) => Promise<MessageResponse> | MessageResponse;

export class MessageBus {
  private readonly runtime: BrowserRuntime;

  constructor(runtime: BrowserRuntime) {
    this.runtime = runtime;
  }

  async send(message: Message): Promise<MessageResponse> {
    try {
      const response = await this.runtime.sendMessage(message);

      return this.parseResponse(response);
    } catch (error) {
      return {
        success: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }

  listen(handler: MessageHandler): void {
    this.runtime.onMessage(async (message, sender) => {
      try {
        return await handler(message as Message, sender);
      } catch (error) {
        return {
          success: false,
          error: error instanceof Error ? error.message : String(error),
        } satisfies MessageResponse;
      }
    });
  }

  private parseResponse(response: unknown): MessageResponse {
    if (
      typeof response === "object" &&
      response !== null &&
      "success" in response &&
      typeof response.success === "boolean"
    ) {
      return response as MessageResponse;
    }

    return {
      success: false,
      error: "Malformed response received from extension.",
    };
  }
}
