export type LogLevel = "debug" | "info" | "warn" | "error";

export interface LogEntry {
  timestamp: string;
  level: LogLevel;
  message: string;
  context?: Record<string, unknown>;
}

export class Logger {
  private readonly namespace: string;

  constructor(namespace = "DSSP") {
    this.namespace = namespace;
  }

  debug(message: string, context?: Record<string, unknown>): void {
    this.write("debug", message, context);
  }

  info(message: string, context?: Record<string, unknown>): void {
    this.write("info", message, context);
  }

  warn(message: string, context?: Record<string, unknown>): void {
    this.write("warn", message, context);
  }

  error(message: string, context?: Record<string, unknown>): void {
    this.write("error", message, context);
  }

  private write(
    level: LogLevel,
    message: string,
    context?: Record<string, unknown>,
  ): void {
    const entry: LogEntry = {
      timestamp: new Date().toISOString(),
      level,
      message,
      context,
    };

    const prefix = `[${this.namespace}]`;

    switch (level) {
      case "debug":
        console.debug(prefix, entry);
        break;
      case "info":
        console.info(prefix, entry);
        break;
      case "warn":
        console.warn(prefix, entry);
        break;
      case "error":
        console.error(prefix, entry);
        break;
    }
  }
}