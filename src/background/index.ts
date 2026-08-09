import { MessageBus } from "../core/infrastructure/messaging/MessageBus";

const messageBus = new MessageBus();

messageBus.listen(async (message) => {
  console.log("[Background] Received:", message);

  switch (message.type) {
    case "GET_STATUS":
      return {
        success: true,
        data: {
          status: "ready",
        },
      };

    case "START_AUTOMATION":
      return {
        success: true,
        data: {
          status: "started",
        },
      };

    case "PAUSE_AUTOMATION":
      return {
        success: true,
        data: {
          status: "paused",
        },
      };

    case "STOP_AUTOMATION":
      return {
        success: true,
        data: {
          status: "stopped",
        },
      };
  }
});