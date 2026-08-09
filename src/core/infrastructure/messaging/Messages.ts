export type Message =
  | {
      type: "GET_STATUS";
    }
  | {
      type: "START_AUTOMATION";
    }
  | {
      type: "PAUSE_AUTOMATION";
    }
  | {
      type: "STOP_AUTOMATION";
    };

export type MessageResponse =
  | {
      success: true;
      data?: unknown;
    }
  | {
      success: false;
      error: string;
    };