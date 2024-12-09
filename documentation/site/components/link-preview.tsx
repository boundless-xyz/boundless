import { cn } from "@risc0/ui/cn";
import { Skeleton } from "@risc0/ui/skeleton";
import { useEffect, useState } from "react";

type PreviewData = {
  title: string;
  description: string;
  image: string;
  url: string;
};

type LinkPreviewProps = {
  url: string;
  className?: string;
  imgClassName?: string;
};

export default function LinkPreview({ url, className = "", imgClassName = "" }: LinkPreviewProps) {
  const [preview, setPreview] = useState<PreviewData | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function loadPreview() {
      try {
        setLoading(true);
        setError(null);

        // Load from cache
        const response = await fetch("/link-previews.json");
        const cache = await response.json();

        if (cache[url]) {
          setPreview(cache[url]);
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load preview");
      } finally {
        setLoading(false);
      }
    }

    if (url) {
      loadPreview();
    }
  }, [url]);

  if (loading) {
    return <Skeleton className="h-52 w-full" />;
  }

  if (error || !preview) {
    return null;
  }

  return (
    <a
      href={preview.url}
      target="_blank"
      rel="noopener noreferrer"
      className={`block overflow-hidden rounded-lg border border-border p-2 no-underline transition-opacity hover:opacity-80 ${className}`}
    >
      <div className="flex flex-row flex-nowrap">
        {preview.image && (
          <img
            src={preview.image}
            alt={preview.title}
            className={cn(`h-48 w-auto object-cover object-center shadow ${imgClassName}`)}
          />
        )}
        <div className="min-w-0 px-8 py-6">
          <h3 className="line-clamp-1 font-bold text-lg">{preview.title}</h3>
          {preview.description && <p className="mt-2 line-clamp-3 text-gray-600 text-sm">{preview.description}</p>}
          <p className="mt-2 truncate text-gray-500 text-xs">{preview.url}</p>
        </div>
      </div>
    </a>
  );
}
