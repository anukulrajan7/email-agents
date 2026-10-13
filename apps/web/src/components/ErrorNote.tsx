import { ApiError } from '../api/client';

export function ErrorNote({ error }: { error: unknown }) {
  if (!error) return null;
  const message = error instanceof ApiError ? error.message : error instanceof Error ? error.message : 'Something went wrong';
  return (
    <p role="alert" className="rounded-sm border-l-[3px] border-danger bg-danger-tint px-3 py-2 text-sm text-danger">
      {message}
    </p>
  );
}
