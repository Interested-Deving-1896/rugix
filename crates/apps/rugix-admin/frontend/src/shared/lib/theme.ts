import type { Theme } from "../../types";

export function initialTheme(): Theme {
  const stored = localStorage.getItem("rugix-admin-theme");
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}
