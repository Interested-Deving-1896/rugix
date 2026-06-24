export function classes(...values: Array<string | false | undefined>) {
  return values.filter(Boolean).join(" ");
}
