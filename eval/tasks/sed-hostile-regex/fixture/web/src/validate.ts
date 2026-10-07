// Client-side mirrors of src/validate/patterns.py. Keep both in sync.

export const SLUG = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
export const UNIX_PATH = /^(?:\/[^/]+)+\/?$/;
export const ORDER_ID = /^ORD-\d{6,8}$/;
export const HEX_COLOR = /^#(?:[0-9a-fA-F]{3}){1,2}$/;
export const EMAIL = /^[^\s@]+@[^\s@]+\.[a-z]{2,}$/i;

export const DIGITS = new RegExp("\\d+", "g");
export const TAG_SPLIT = new RegExp("\\s*\\|\\s*");
export const WIN_DRIVE = new RegExp("^[A-Za-z]:\\\\");

export function formatPhone(value: string): string {
  return value.replace(/^\+?1?[ .-]?\(?(\d{3})\)?[ .-]?(\d{3})[ .-]?(\d{4})$/, "($1) $2-$3");
}

export function highlightMatches(text: string, term: RegExp): string {
  return text.replace(term, "<mark>$&</mark>");
}

export function collapseSlashes(path: string): string {
  return path.replace(/\/{2,}/g, "/");
}

export function splitTags(input: string): string[] {
  return input.split(TAG_SPLIT).filter((tag) => /^[\w.-]+$/.test(tag));
}
