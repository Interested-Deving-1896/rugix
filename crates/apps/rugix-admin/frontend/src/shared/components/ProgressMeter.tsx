export function ProgressMeter({ percent, fallback }: { percent?: number; fallback?: string }) {
  if (percent === undefined && !fallback) return null;
  return (
    <div>
      <div className="mb-1 flex justify-between text-xs text-foreground-muted">
        <span>Upload</span>
        <span>{percent === undefined ? fallback : `${percent}%`}</span>
      </div>
      {percent !== undefined && (
        <div className="h-1.5 overflow-hidden rounded-full bg-elevation-3">
          <div className="h-full bg-primary" style={{ width: `${percent}%` }} />
        </div>
      )}
    </div>
  );
}
