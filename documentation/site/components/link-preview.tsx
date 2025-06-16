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
};

export default function LinkPreview({ url, className = "" }: LinkPreviewProps) {
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

  if (loading || error || !preview) {
    return null;
  }

  return (
    <a
      href={preview.url}
      target="_blank"
      rel="noopener noreferrer"
      style={{
        display: "block",
        overflow: "hidden",
        borderRadius: "0.5rem",
        border: "1px solid var(--border)",
        textDecoration: "none",
        transition: "opacity 150ms",
        ...(className && typeof className === "object" ? className : {}),
      }}
      onMouseOver={(e) => {
        e.currentTarget.style.opacity = "0.8";
      }}
      onMouseOut={(e) => {
        e.currentTarget.style.opacity = "1";
      }}
      onFocus={(e) => {
        e.currentTarget.style.opacity = "0.8";
      }}
      onBlur={(e) => {
        e.currentTarget.style.opacity = "1";
      }}
    >
      <div style={{ display: "flex", flexDirection: "row", flexWrap: "nowrap" }}>
        {preview.image && (
          <img
            src={preview.image}
            alt={preview.title}
            style={{
              height: "12rem",
              width: "auto",
              objectFit: "cover",
              objectPosition: "center",
              boxShadow: "0 1px 3px 0 rgb(0 0 0 / 0.1)",
              maxWidth: "364px",
            }}
          />
        )}
        <div style={{ minWidth: 0, padding: "1.5rem 2rem" }}>
          <h3
            style={{
              fontWeight: "bold",
              fontSize: "1.125rem",
              display: "-webkit-box",
              WebkitLineClamp: 1,
              WebkitBoxOrient: "vertical",
              overflow: "hidden",
            }}
          >
            {preview.title}
          </h3>
          {preview.description && (
            <p
              style={{
                marginTop: "0.5rem",
                color: "rgb(75 85 99)",
                fontSize: "0.875rem",
                display: "-webkit-box",
                WebkitLineClamp: 3,
                WebkitBoxOrient: "vertical",
                overflow: "hidden",
              }}
            >
              {preview.description}
            </p>
          )}
          <p
            style={{
              marginTop: "0.5rem",
              color: "rgb(107 114 128)",
              fontSize: "0.75rem",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {preview.url}
          </p>
        </div>
      </div>
    </a>
  );
}
