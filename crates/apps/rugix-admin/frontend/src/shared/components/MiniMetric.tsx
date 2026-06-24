export function MiniMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-divider bg-elevation-0 px-3 py-2">
      <div className="text-xs font-semibold uppercase tracking-wide text-foreground-subtle">{label}</div>
      <div className="mt-1 truncate text-sm font-medium">{value}</div>
    </div>
  );
}
