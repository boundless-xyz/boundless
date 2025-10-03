import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const queryClient = new QueryClient();

export default function RootLayout({ children }) {
  return (
    <>
      {typeof window !== "undefined" && (
        <link rel="canonical" href={`https://docs.boundless.network${window.location.pathname}`} />
      )}

      {/* Custom JS scripts */}
      <script defer data-domain="docs.beboundless.xyz" src="https://plausible.io/js/script.outbound-links.js" />

      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    </>
  );
}
