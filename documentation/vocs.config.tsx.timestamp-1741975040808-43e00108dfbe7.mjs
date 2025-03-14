// vocs.config.tsx
import path from "node:path";
import "file:///Users/cohan/code/boundless/documentation/node_modules/react/index.js";
import biomePlugin from "file:///Users/cohan/code/boundless/documentation/node_modules/vite-plugin-biome/dist/index.mjs";
import VitePluginSitemap from "file:///Users/cohan/code/boundless/documentation/node_modules/vite-plugin-sitemap/dist/index.js";
import { defineConfig } from "file:///Users/cohan/code/boundless/documentation/node_modules/vocs/_lib/index.js";
var SIDEBAR_CONFIG = [
  {
    text: "\u2728 Introduction",
    items: [
      {
        text: "Why Boundless?",
        link: "/introduction/why-boundless"
      },
      {
        text: "What is Boundless?",
        link: "/introduction/what-is-boundless",
        collapsed: true,
        items: [
          {
            text: "Extensions",
            link: "/introduction/extensions"
          }
        ]
      },
      {
        text: "Proof Lifecycle",
        link: "/introduction/proof-lifecycle"
      }
    ]
  },
  {
    text: "\u{1F3CB}\uFE0F Build",
    items: [
      {
        text: "Build a Program",
        link: "/build/build-a-program"
      },
      {
        text: "Request a Proof",
        link: "/build/request-a-proof",
        collapsed: true,
        items: [
          {
            text: "Pricing a Request",
            link: "/build/pricing-a-request"
          },
          {
            text: "Troubleshooting",
            link: "/build/troubleshooting-a-request"
          }
        ]
      },
      {
        text: "Use a Proof",
        link: "/build/use-a-proof"
      }
    ]
  },
  {
    text: "\u{1F9EA} Prove",
    items: [
      {
        text: "Becoming a Prover",
        link: "/prove/becoming-a-prover"
      },
      {
        text: "Requirements",
        link: "/prove/requirements"
      },
      {
        text: "Quick Start",
        link: "/prove/quick-start"
      },
      {
        text: "Running a Boundless Prover",
        link: "/prove/proving-stack",
        collapsed: true,
        items: [
          {
            text: "The Boundless Proving Stack",
            link: "/prove/proving-stack"
          },
          {
            text: "Broker Configuration & Operation",
            link: "/prove/broker"
          },
          {
            text: "Monitoring",
            link: "/prove/monitoring"
          },
          {
            text: "Performance Optimization",
            link: "/prove/performance-optimization"
          }
        ]
      }
    ]
  },
  {
    text: "\u{1F9E0} Advanced & References",
    items: [
      {
        text: "Deployments",
        link: "/deployments"
      },
      {
        text: "Smart Contracts",
        link: "/smart-contracts"
      },
      {
        text: "Terminology",
        link: "/terminology"
      },
      {
        text: "Bento Technical Design",
        link: "/bento-technical-design"
      }
    ]
  }
];
function generateSitemap() {
  function extractRoutes(items) {
    return items.flatMap((item) => {
      const routes = [];
      if (item.link) {
        routes.push(item.link);
      }
      if (item.items) {
        routes.push(...extractRoutes(item.items));
      }
      return routes;
    });
  }
  return VitePluginSitemap({
    hostname: "https://docs.beboundless.xyz",
    dynamicRoutes: extractRoutes(SIDEBAR_CONFIG),
    changefreq: "weekly",
    outDir: "site/dist"
  });
}
var vocs_config_default = defineConfig({
  logoUrl: "/logo.svg",
  font: {
    mono: {
      google: "Ubuntu Mono"
    }
  },
  vite: {
    plugins: [generateSitemap(), biomePlugin()],
    resolve: {
      alias: {
        "lightgallery/fonts": path.resolve(__vite_injected_original_dirname, "node_modules/lightgallery/fonts"),
        "lightgallery/images": path.resolve(__vite_injected_original_dirname, "node_modules/lightgallery/images")
      }
    },
    server: {
      fs: {
        allow: ["node_modules/lightgallery"]
      }
    }
  },
  sidebar: SIDEBAR_CONFIG,
  topNav: [
    { text: "Explorer", link: "https://explorer.beboundless.xyz" },
    { text: "Help", link: "https://t.me/+E9J7zgtyoTVlNzk1" }
    /*{
      text: process.env.LATEST_TAG || "Latest",
      items: [
        {
          text: "Releases",
          link: "https://github.com/boundless-xyz/boundless/releases",
        },
      ],
    },*/
  ],
  socials: [
    /*{
      icon: "github",
      link: "https://github.com/boundless-xyz",
    },*/
    {
      icon: "x",
      link: "https://x.com/boundless_xyz"
    }
  ],
  rootDir: "site",
  title: "Boundless Docs",
  /*logoUrl: {
    light: "/logo.png",
    dark: "/logo-dark.png",
  },*/
  theme: {
    accentColor: {
      light: "#537263",
      // Forest - primary accent for light mode
      dark: "#AED8C4"
      // Leaf - lighter accent for dark mode
    },
    variables: {
      color: {
        backgroundDark: {
          light: "#EFECE3",
          // Sand
          dark: "#1e1d1f"
        },
        background: {
          light: "#FFFFFF",
          dark: "#232225"
        }
      },
      content: {
        width: "calc(90ch + (var(--vocs-content_horizontalPadding) * 2))"
      }
    }
  },
  iconUrl: {
    light: "/favicon.svg",
    dark: "/favicon.svg"
  },
  banner: {
    dismissable: true,
    content: "BREAKING: Boundless is opening the allowlist for infrastructure companies to start proving, please fill out this [form](https://docs.google.com/forms/d/e/1FAIpQLScr5B3TZfzLKIb0Hk6oqiMMXdRh4cwpTlczi_zGqdwabvbrfw/viewform) to apply for access. See the new [proving docs](/prove/becoming-a-prover) for more info."
  },
  ogImageUrl: "https://docs.beboundless.xyz/og.png"
});
export {
  vocs_config_default as default,
  generateSitemap
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidm9jcy5jb25maWcudHN4Il0sCiAgInNvdXJjZXNDb250ZW50IjogWyJpbXBvcnQgcGF0aCBmcm9tIFwibm9kZTpwYXRoXCI7XG5pbXBvcnQgKiBhcyBSZWFjdCBmcm9tIFwicmVhY3RcIjtcbmltcG9ydCBiaW9tZVBsdWdpbiBmcm9tIFwidml0ZS1wbHVnaW4tYmlvbWVcIjtcbmltcG9ydCBWaXRlUGx1Z2luU2l0ZW1hcCBmcm9tIFwidml0ZS1wbHVnaW4tc2l0ZW1hcFwiO1xuaW1wb3J0IHsgZGVmaW5lQ29uZmlnIH0gZnJvbSBcInZvY3NcIjtcblxuY29uc3QgU0lERUJBUl9DT05GSUcgPSBbXG4gIHtcbiAgICB0ZXh0OiBcIlx1MjcyOCBJbnRyb2R1Y3Rpb25cIixcbiAgICBpdGVtczogW1xuICAgICAge1xuICAgICAgICB0ZXh0OiBcIldoeSBCb3VuZGxlc3M/XCIsXG4gICAgICAgIGxpbms6IFwiL2ludHJvZHVjdGlvbi93aHktYm91bmRsZXNzXCIsXG4gICAgICB9LFxuICAgICAge1xuICAgICAgICB0ZXh0OiBcIldoYXQgaXMgQm91bmRsZXNzP1wiLFxuICAgICAgICBsaW5rOiBcIi9pbnRyb2R1Y3Rpb24vd2hhdC1pcy1ib3VuZGxlc3NcIixcbiAgICAgICAgY29sbGFwc2VkOiB0cnVlLFxuICAgICAgICBpdGVtczogW1xuICAgICAgICAgIHtcbiAgICAgICAgICAgIHRleHQ6IFwiRXh0ZW5zaW9uc1wiLFxuICAgICAgICAgICAgbGluazogXCIvaW50cm9kdWN0aW9uL2V4dGVuc2lvbnNcIixcbiAgICAgICAgICB9LFxuICAgICAgICBdLFxuICAgICAgfSxcbiAgICAgIHtcbiAgICAgICAgdGV4dDogXCJQcm9vZiBMaWZlY3ljbGVcIixcbiAgICAgICAgbGluazogXCIvaW50cm9kdWN0aW9uL3Byb29mLWxpZmVjeWNsZVwiLFxuICAgICAgfSxcbiAgICBdLFxuICB9LFxuICB7XG4gICAgdGV4dDogXCJcdUQ4M0NcdURGQ0JcdUZFMEYgQnVpbGRcIixcbiAgICBpdGVtczogW1xuICAgICAge1xuICAgICAgICB0ZXh0OiBcIkJ1aWxkIGEgUHJvZ3JhbVwiLFxuICAgICAgICBsaW5rOiBcIi9idWlsZC9idWlsZC1hLXByb2dyYW1cIixcbiAgICAgIH0sXG4gICAgICB7XG4gICAgICAgIHRleHQ6IFwiUmVxdWVzdCBhIFByb29mXCIsXG4gICAgICAgIGxpbms6IFwiL2J1aWxkL3JlcXVlc3QtYS1wcm9vZlwiLFxuICAgICAgICBjb2xsYXBzZWQ6IHRydWUsXG4gICAgICAgIGl0ZW1zOiBbXG4gICAgICAgICAge1xuICAgICAgICAgICAgdGV4dDogXCJQcmljaW5nIGEgUmVxdWVzdFwiLFxuICAgICAgICAgICAgbGluazogXCIvYnVpbGQvcHJpY2luZy1hLXJlcXVlc3RcIixcbiAgICAgICAgICB9LFxuICAgICAgICAgIHtcbiAgICAgICAgICAgIHRleHQ6IFwiVHJvdWJsZXNob290aW5nXCIsXG4gICAgICAgICAgICBsaW5rOiBcIi9idWlsZC90cm91Ymxlc2hvb3RpbmctYS1yZXF1ZXN0XCIsXG4gICAgICAgICAgfSxcbiAgICAgICAgXSxcbiAgICAgIH0sXG4gICAgICB7XG4gICAgICAgIHRleHQ6IFwiVXNlIGEgUHJvb2ZcIixcbiAgICAgICAgbGluazogXCIvYnVpbGQvdXNlLWEtcHJvb2ZcIixcbiAgICAgIH0sXG4gICAgXSxcbiAgfSxcbiAge1xuICAgIHRleHQ6IFwiXHVEODNFXHVEREVBIFByb3ZlXCIsXG4gICAgaXRlbXM6IFtcbiAgICAgIHtcbiAgICAgICAgdGV4dDogXCJCZWNvbWluZyBhIFByb3ZlclwiLFxuICAgICAgICBsaW5rOiBcIi9wcm92ZS9iZWNvbWluZy1hLXByb3ZlclwiLFxuICAgICAgfSxcbiAgICAgIHtcbiAgICAgICAgdGV4dDogXCJSZXF1aXJlbWVudHNcIixcbiAgICAgICAgbGluazogXCIvcHJvdmUvcmVxdWlyZW1lbnRzXCIsXG4gICAgICB9LFxuICAgICAge1xuICAgICAgICB0ZXh0OiBcIlF1aWNrIFN0YXJ0XCIsXG4gICAgICAgIGxpbms6IFwiL3Byb3ZlL3F1aWNrLXN0YXJ0XCIsXG4gICAgICB9LFxuICAgICAge1xuICAgICAgICB0ZXh0OiBcIlJ1bm5pbmcgYSBCb3VuZGxlc3MgUHJvdmVyXCIsXG4gICAgICAgIGxpbms6IFwiL3Byb3ZlL3Byb3Zpbmctc3RhY2tcIixcbiAgICAgICAgY29sbGFwc2VkOiB0cnVlLFxuICAgICAgICBpdGVtczogW1xuICAgICAgICAgIHtcbiAgICAgICAgICAgIHRleHQ6IFwiVGhlIEJvdW5kbGVzcyBQcm92aW5nIFN0YWNrXCIsXG4gICAgICAgICAgICBsaW5rOiBcIi9wcm92ZS9wcm92aW5nLXN0YWNrXCIsXG4gICAgICAgICAgfSxcbiAgICAgICAgICB7XG4gICAgICAgICAgICB0ZXh0OiBcIkJyb2tlciBDb25maWd1cmF0aW9uICYgT3BlcmF0aW9uXCIsXG4gICAgICAgICAgICBsaW5rOiBcIi9wcm92ZS9icm9rZXJcIixcbiAgICAgICAgICB9LFxuICAgICAgICAgIHtcbiAgICAgICAgICAgIHRleHQ6IFwiTW9uaXRvcmluZ1wiLFxuICAgICAgICAgICAgbGluazogXCIvcHJvdmUvbW9uaXRvcmluZ1wiLFxuICAgICAgICAgIH0sXG4gICAgICAgICAge1xuICAgICAgICAgICAgdGV4dDogXCJQZXJmb3JtYW5jZSBPcHRpbWl6YXRpb25cIixcbiAgICAgICAgICAgIGxpbms6IFwiL3Byb3ZlL3BlcmZvcm1hbmNlLW9wdGltaXphdGlvblwiLFxuICAgICAgICAgIH0sXG4gICAgICAgIF0sXG4gICAgICB9LFxuICAgIF0sXG4gIH0sXG4gIHtcbiAgICB0ZXh0OiBcIlx1RDgzRVx1RERFMCBBZHZhbmNlZCAmIFJlZmVyZW5jZXNcIixcbiAgICBpdGVtczogW1xuICAgICAge1xuICAgICAgICB0ZXh0OiBcIkRlcGxveW1lbnRzXCIsXG4gICAgICAgIGxpbms6IFwiL2RlcGxveW1lbnRzXCIsXG4gICAgICB9LFxuICAgICAge1xuICAgICAgICB0ZXh0OiBcIlNtYXJ0IENvbnRyYWN0c1wiLFxuICAgICAgICBsaW5rOiBcIi9zbWFydC1jb250cmFjdHNcIixcbiAgICAgIH0sXG4gICAgICB7XG4gICAgICAgIHRleHQ6IFwiVGVybWlub2xvZ3lcIixcbiAgICAgICAgbGluazogXCIvdGVybWlub2xvZ3lcIixcbiAgICAgIH0sXG4gICAgICB7XG4gICAgICAgIHRleHQ6IFwiQmVudG8gVGVjaG5pY2FsIERlc2lnblwiLFxuICAgICAgICBsaW5rOiBcIi9iZW50by10ZWNobmljYWwtZGVzaWduXCIsXG4gICAgICB9LFxuICAgIF0sXG4gIH0sXG5dO1xuXG5leHBvcnQgZnVuY3Rpb24gZ2VuZXJhdGVTaXRlbWFwKCkge1xuICBmdW5jdGlvbiBleHRyYWN0Um91dGVzKGl0ZW1zKTogc3RyaW5nW10ge1xuICAgIHJldHVybiBpdGVtcy5mbGF0TWFwKChpdGVtKSA9PiB7XG4gICAgICBjb25zdCByb3V0ZXM6IHN0cmluZ1tdID0gW107XG5cbiAgICAgIGlmIChpdGVtLmxpbmspIHtcbiAgICAgICAgcm91dGVzLnB1c2goaXRlbS5saW5rKTtcbiAgICAgIH1cblxuICAgICAgaWYgKGl0ZW0uaXRlbXMpIHtcbiAgICAgICAgcm91dGVzLnB1c2goLi4uZXh0cmFjdFJvdXRlcyhpdGVtLml0ZW1zKSk7XG4gICAgICB9XG5cbiAgICAgIHJldHVybiByb3V0ZXM7XG4gICAgfSk7XG4gIH1cblxuICByZXR1cm4gVml0ZVBsdWdpblNpdGVtYXAoe1xuICAgIGhvc3RuYW1lOiBcImh0dHBzOi8vZG9jcy5iZWJvdW5kbGVzcy54eXpcIixcbiAgICBkeW5hbWljUm91dGVzOiBleHRyYWN0Um91dGVzKFNJREVCQVJfQ09ORklHKSxcbiAgICBjaGFuZ2VmcmVxOiBcIndlZWtseVwiLFxuICAgIG91dERpcjogXCJzaXRlL2Rpc3RcIixcbiAgfSk7XG59XG5cbmV4cG9ydCBkZWZhdWx0IGRlZmluZUNvbmZpZyh7XG4gIGxvZ29Vcmw6IFwiL2xvZ28uc3ZnXCIsXG4gIGZvbnQ6IHtcbiAgICBtb25vOiB7XG4gICAgICBnb29nbGU6IFwiVWJ1bnR1IE1vbm9cIixcbiAgICB9LFxuICB9LFxuICB2aXRlOiB7XG4gICAgcGx1Z2luczogW2dlbmVyYXRlU2l0ZW1hcCgpLCBiaW9tZVBsdWdpbigpXSxcbiAgICByZXNvbHZlOiB7XG4gICAgICBhbGlhczoge1xuICAgICAgICBcImxpZ2h0Z2FsbGVyeS9mb250c1wiOiBwYXRoLnJlc29sdmUoX19kaXJuYW1lLCBcIm5vZGVfbW9kdWxlcy9saWdodGdhbGxlcnkvZm9udHNcIiksXG4gICAgICAgIFwibGlnaHRnYWxsZXJ5L2ltYWdlc1wiOiBwYXRoLnJlc29sdmUoX19kaXJuYW1lLCBcIm5vZGVfbW9kdWxlcy9saWdodGdhbGxlcnkvaW1hZ2VzXCIpLFxuICAgICAgfSxcbiAgICB9LFxuICAgIHNlcnZlcjoge1xuICAgICAgZnM6IHtcbiAgICAgICAgYWxsb3c6IFtcIm5vZGVfbW9kdWxlcy9saWdodGdhbGxlcnlcIl0sXG4gICAgICB9LFxuICAgIH0sXG4gIH0sXG4gIHNpZGViYXI6IFNJREVCQVJfQ09ORklHLFxuICB0b3BOYXY6IFtcbiAgICB7IHRleHQ6IFwiRXhwbG9yZXJcIiwgbGluazogXCJodHRwczovL2V4cGxvcmVyLmJlYm91bmRsZXNzLnh5elwiIH0sXG4gICAgeyB0ZXh0OiBcIkhlbHBcIiwgbGluazogXCJodHRwczovL3QubWUvK0U5Sjd6Z3R5b1RWbE56azFcIiB9LFxuICAgIC8qe1xuICAgICAgdGV4dDogcHJvY2Vzcy5lbnYuTEFURVNUX1RBRyB8fCBcIkxhdGVzdFwiLFxuICAgICAgaXRlbXM6IFtcbiAgICAgICAge1xuICAgICAgICAgIHRleHQ6IFwiUmVsZWFzZXNcIixcbiAgICAgICAgICBsaW5rOiBcImh0dHBzOi8vZ2l0aHViLmNvbS9ib3VuZGxlc3MteHl6L2JvdW5kbGVzcy9yZWxlYXNlc1wiLFxuICAgICAgICB9LFxuICAgICAgXSxcbiAgICB9LCovXG4gIF0sXG4gIHNvY2lhbHM6IFtcbiAgICAvKntcbiAgICAgIGljb246IFwiZ2l0aHViXCIsXG4gICAgICBsaW5rOiBcImh0dHBzOi8vZ2l0aHViLmNvbS9ib3VuZGxlc3MteHl6XCIsXG4gICAgfSwqL1xuICAgIHtcbiAgICAgIGljb246IFwieFwiLFxuICAgICAgbGluazogXCJodHRwczovL3guY29tL2JvdW5kbGVzc194eXpcIixcbiAgICB9LFxuICBdLFxuICByb290RGlyOiBcInNpdGVcIixcbiAgdGl0bGU6IFwiQm91bmRsZXNzIERvY3NcIixcbiAgLypsb2dvVXJsOiB7XG4gICAgbGlnaHQ6IFwiL2xvZ28ucG5nXCIsXG4gICAgZGFyazogXCIvbG9nby1kYXJrLnBuZ1wiLFxuICB9LCovXG4gIHRoZW1lOiB7XG4gICAgYWNjZW50Q29sb3I6IHtcbiAgICAgIGxpZ2h0OiBcIiM1MzcyNjNcIiwgLy8gRm9yZXN0IC0gcHJpbWFyeSBhY2NlbnQgZm9yIGxpZ2h0IG1vZGVcbiAgICAgIGRhcms6IFwiI0FFRDhDNFwiLCAvLyBMZWFmIC0gbGlnaHRlciBhY2NlbnQgZm9yIGRhcmsgbW9kZVxuICAgIH0sXG4gICAgdmFyaWFibGVzOiB7XG4gICAgICBjb2xvcjoge1xuICAgICAgICBiYWNrZ3JvdW5kRGFyazoge1xuICAgICAgICAgIGxpZ2h0OiBcIiNFRkVDRTNcIiwgLy8gU2FuZFxuICAgICAgICAgIGRhcms6IFwiIzFlMWQxZlwiLFxuICAgICAgICB9LFxuICAgICAgICBiYWNrZ3JvdW5kOiB7XG4gICAgICAgICAgbGlnaHQ6IFwiI0ZGRkZGRlwiLFxuICAgICAgICAgIGRhcms6IFwiIzIzMjIyNVwiLFxuICAgICAgICB9LFxuICAgICAgfSxcbiAgICAgIGNvbnRlbnQ6IHtcbiAgICAgICAgd2lkdGg6IFwiY2FsYyg5MGNoICsgKHZhcigtLXZvY3MtY29udGVudF9ob3Jpem9udGFsUGFkZGluZykgKiAyKSlcIixcbiAgICAgIH0sXG4gICAgfSxcbiAgfSxcbiAgaWNvblVybDoge1xuICAgIGxpZ2h0OiBcIi9mYXZpY29uLnN2Z1wiLFxuICAgIGRhcms6IFwiL2Zhdmljb24uc3ZnXCIsXG4gIH0sXG4gIGJhbm5lcjoge1xuICAgIGRpc21pc3NhYmxlOiB0cnVlLFxuICAgIGNvbnRlbnQ6XG4gICAgICBcIkJSRUFLSU5HOiBCb3VuZGxlc3MgaXMgb3BlbmluZyB0aGUgYWxsb3dsaXN0IGZvciBpbmZyYXN0cnVjdHVyZSBjb21wYW5pZXMgdG8gc3RhcnQgcHJvdmluZywgcGxlYXNlIGZpbGwgb3V0IHRoaXMgW2Zvcm1dKGh0dHBzOi8vZG9jcy5nb29nbGUuY29tL2Zvcm1zL2QvZS8xRkFJcFFMU2NyNUIzVFpmekxLSWIwSGs2b3FpTU1YZFJoNGN3cFRsY3ppX3pHcWR3YWJ2YnJmdy92aWV3Zm9ybSkgdG8gYXBwbHkgZm9yIGFjY2Vzcy4gU2VlIHRoZSBuZXcgW3Byb3ZpbmcgZG9jc10oL3Byb3ZlL2JlY29taW5nLWEtcHJvdmVyKSBmb3IgbW9yZSBpbmZvLlwiLFxuICB9LFxuICBvZ0ltYWdlVXJsOiBcImh0dHBzOi8vZG9jcy5iZWJvdW5kbGVzcy54eXovb2cucG5nXCIsXG59KTtcbiJdLAogICJtYXBwaW5ncyI6ICI7QUFBQSxPQUFPLFVBQVU7QUFDakIsT0FBdUI7QUFDdkIsT0FBTyxpQkFBaUI7QUFDeEIsT0FBTyx1QkFBdUI7QUFDOUIsU0FBUyxvQkFBb0I7QUFFN0IsSUFBTSxpQkFBaUI7QUFBQSxFQUNyQjtBQUFBLElBQ0UsTUFBTTtBQUFBLElBQ04sT0FBTztBQUFBLE1BQ0w7QUFBQSxRQUNFLE1BQU07QUFBQSxRQUNOLE1BQU07QUFBQSxNQUNSO0FBQUEsTUFDQTtBQUFBLFFBQ0UsTUFBTTtBQUFBLFFBQ04sTUFBTTtBQUFBLFFBQ04sV0FBVztBQUFBLFFBQ1gsT0FBTztBQUFBLFVBQ0w7QUFBQSxZQUNFLE1BQU07QUFBQSxZQUNOLE1BQU07QUFBQSxVQUNSO0FBQUEsUUFDRjtBQUFBLE1BQ0Y7QUFBQSxNQUNBO0FBQUEsUUFDRSxNQUFNO0FBQUEsUUFDTixNQUFNO0FBQUEsTUFDUjtBQUFBLElBQ0Y7QUFBQSxFQUNGO0FBQUEsRUFDQTtBQUFBLElBQ0UsTUFBTTtBQUFBLElBQ04sT0FBTztBQUFBLE1BQ0w7QUFBQSxRQUNFLE1BQU07QUFBQSxRQUNOLE1BQU07QUFBQSxNQUNSO0FBQUEsTUFDQTtBQUFBLFFBQ0UsTUFBTTtBQUFBLFFBQ04sTUFBTTtBQUFBLFFBQ04sV0FBVztBQUFBLFFBQ1gsT0FBTztBQUFBLFVBQ0w7QUFBQSxZQUNFLE1BQU07QUFBQSxZQUNOLE1BQU07QUFBQSxVQUNSO0FBQUEsVUFDQTtBQUFBLFlBQ0UsTUFBTTtBQUFBLFlBQ04sTUFBTTtBQUFBLFVBQ1I7QUFBQSxRQUNGO0FBQUEsTUFDRjtBQUFBLE1BQ0E7QUFBQSxRQUNFLE1BQU07QUFBQSxRQUNOLE1BQU07QUFBQSxNQUNSO0FBQUEsSUFDRjtBQUFBLEVBQ0Y7QUFBQSxFQUNBO0FBQUEsSUFDRSxNQUFNO0FBQUEsSUFDTixPQUFPO0FBQUEsTUFDTDtBQUFBLFFBQ0UsTUFBTTtBQUFBLFFBQ04sTUFBTTtBQUFBLE1BQ1I7QUFBQSxNQUNBO0FBQUEsUUFDRSxNQUFNO0FBQUEsUUFDTixNQUFNO0FBQUEsTUFDUjtBQUFBLE1BQ0E7QUFBQSxRQUNFLE1BQU07QUFBQSxRQUNOLE1BQU07QUFBQSxNQUNSO0FBQUEsTUFDQTtBQUFBLFFBQ0UsTUFBTTtBQUFBLFFBQ04sTUFBTTtBQUFBLFFBQ04sV0FBVztBQUFBLFFBQ1gsT0FBTztBQUFBLFVBQ0w7QUFBQSxZQUNFLE1BQU07QUFBQSxZQUNOLE1BQU07QUFBQSxVQUNSO0FBQUEsVUFDQTtBQUFBLFlBQ0UsTUFBTTtBQUFBLFlBQ04sTUFBTTtBQUFBLFVBQ1I7QUFBQSxVQUNBO0FBQUEsWUFDRSxNQUFNO0FBQUEsWUFDTixNQUFNO0FBQUEsVUFDUjtBQUFBLFVBQ0E7QUFBQSxZQUNFLE1BQU07QUFBQSxZQUNOLE1BQU07QUFBQSxVQUNSO0FBQUEsUUFDRjtBQUFBLE1BQ0Y7QUFBQSxJQUNGO0FBQUEsRUFDRjtBQUFBLEVBQ0E7QUFBQSxJQUNFLE1BQU07QUFBQSxJQUNOLE9BQU87QUFBQSxNQUNMO0FBQUEsUUFDRSxNQUFNO0FB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
