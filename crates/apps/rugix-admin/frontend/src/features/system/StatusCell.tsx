export function StatusCell({ label, value }: { label: string; value: string }) {
  return (
    <div className="px-4 py-4">
      <div className="text-xs font-semibold uppercase tracking-wide text-foreground-subtle">{label}</div>
      <div className="mt-1 truncate font-display text-xl font-semibold">{value}</div>
    </div>
  );
}
