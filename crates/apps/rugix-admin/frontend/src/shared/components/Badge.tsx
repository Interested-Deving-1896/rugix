import type { ReactNode } from "react";

export function Badge({ color, children }: { color: string; children: ReactNode }) {
  return (
    <span className={`inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2 py-0.5 text-xs font-medium ring-1 ring-inset ${color}`}>
      {children}
    </span>
  );
}
