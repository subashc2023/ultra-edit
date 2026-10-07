// Display helpers. Validation lives in validate.ts.

export function formatPhoneLoose(value: string): string {
  return value.replace(/(\d{3})(\d{3})(\d{4})/, "($1) $2-$3");
}

export function bold(text: string, term: RegExp): string {
  return text.replace(term, "<b>$&</b>");
}

export function trimSlashes(path: string): string {
  return path.replace(/^\/+|\/+$/g, "");
}
