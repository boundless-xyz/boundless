import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import lightGallery from "lightgallery";
import { useEffect } from "react";
import { http, WagmiProvider, createConfig } from "wagmi";
import { sepolia } from "wagmi/chains";

import "lightgallery/css/lightgallery.css";

const config = createConfig({
  chains: [sepolia],
  transports: {
    [sepolia.id]: http(),
  },
});

const queryClient = new QueryClient();

export default function RootLayout({ children }) {
  useEffect(() => {
    const galleryElement = document.getElementsByClassName("lightgallery");

    if (galleryElement) {
      lightGallery(galleryElement[0] as HTMLElement);
    }

    // Check if we're on the homepage
    if (window.location.pathname === "/") {
      const hasVisitedBefore = localStorage.getItem("hasVisitedDocs");

      if (hasVisitedBefore) {
        window.location.href = "/build/build-a-program";
      } else {
        localStorage.setItem("hasVisitedDocs", "true");
      }
    }
  }, []);

  return (
    <>
      {/* Custom JS scripts */}
      <script defer data-domain="docs.beboundless.xyz" src="https://plausible.io/js/script.outbound-links.js" />

      <WagmiProvider config={config}>
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      </WagmiProvider>
    </>
  );
}
