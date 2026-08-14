import { ValidationError } from "./errors";

const ISO_DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;
const YEAR_FIRST_PATTERN = /^(\d{4})(?:\/)([0-1]?\d)(?:\/)([0-3]?\d)$/;
const DAY_FIRST_PATTERN = /^(\d{1,2})(?:\/|-)(\d{1,2})(?:\/|-)(\d{4})$/;

function checkedDate(year: number, month: number, day: number): string {
  if (year < 1000 || year > 9999 || month < 1 || month > 12) {
    throw new ValidationError("Training date must be a real calendar date.");
  }

  const candidate = new Date(Date.UTC(year, month - 1, day));

  if (
    candidate.getUTCFullYear() !== year ||
    candidate.getUTCMonth() !== month - 1 ||
    candidate.getUTCDate() !== day
  ) {
    throw new ValidationError("Training date must be a real calendar date.");
  }

  return `${year.toString().padStart(4, "0")}-${month
    .toString()
    .padStart(2, "0")}-${day.toString().padStart(2, "0")}`;
}

export function formatTrainingDate(value: string): string {
  const input = value.trim();

  let match = ISO_DATE_PATTERN.exec(input);

  if (match) {
    return checkedDate(Number(match[1]), Number(match[2]), Number(match[3]));
  }

  match = YEAR_FIRST_PATTERN.exec(input);

  if (match) {
    return checkedDate(Number(match[1]), Number(match[2]), Number(match[3]));
  }

  match = DAY_FIRST_PATTERN.exec(input);

  if (match) {
    return checkedDate(Number(match[3]), Number(match[2]), Number(match[1]));
  }

  throw new ValidationError(
    "Training date must use YYYY-MM-DD (or DD/MM/YYYY) format.",
  );
}
