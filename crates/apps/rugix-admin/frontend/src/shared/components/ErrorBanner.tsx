export function ErrorBanner({ message }: { message: string }) {
  return (
    <div className="rounded-lg border border-danger/30 bg-danger-surface px-4 py-3 text-sm text-danger">
      {message}
    </div>
  );
}
