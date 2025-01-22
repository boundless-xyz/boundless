import path from "node:path";
import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SIDEBAR_CONFIG = [
  {
    text: "✨ Introduction",
    items: [
      {
        text: "Why Boundless?",
        link: "/introduction/why-boundless",
      },
      {
        text: "What is Boundless?",
        link: "/introduction/what-is-boundless",
        collapsed: true,
        items: [
          {
            text: "Extensions",
            link: "/introduction/extensions",
          },
        ],
      },
      {
        text: "Proof Lifecycle",
        link: "/introduction/proof-lifecycle",
      },
    ],
  },
  {
    text: "🏋️ Build",
    items: [
      {
        text: "Build a Program",
        link: "/build/build-a-program",
      },
      {
        text: "Request a Proof",
        link: "/build/request-a-proof",
        collapsed: true,
        items: [
          {
            text: "Pricing a Request",
            link: "/build/pricing-a-request",
          },
          {
            text: "Troubleshooting",
            link: "/build/troubleshooting-a-request",
          },
        ]
      },
      {
        text: "Use a Proof",
        link: "/build/use-a-proof",
      },
    ],
  },
  {
    text: "🧪 Prove",
    items: [
      {
        text: "Requirements",
        link: "/prove/requirements",
      },
      {
        text: "Quick Start",
        link: "/prove/quick-start",
      },
      {
        text: "Running a Boundless Prover",
        link: "/prove/proving-stack",
        collapsed: true,
        items: [
          {
            text: "The Boundless Proving Stack", 
            link: "/prove/proving-stack",
          },
          {
            text: "Broker Configuration & Operation",
            link: "/prove/broker",
          },
          {
            text: "Monitoring",
            link: "/prove/monitoring",
          },
          {
            text: "Performance Optimization",
            link: "/prove/performance-optimization",
          },
        ]
      },
    ],
  },
  {
    text: "🧠 Advanced & References",
    items: [
      {
        text: "Deployments",
        link: "/deployments",
      },
      {
        text: "Smart Contracts",
        link: "/smart-contracts",
      },
      {
        text: "Terminology",
        link: "/terminology",
      },
      {
        text: "Bento Technical Design", 
        link: "/bento-technical-design",
      },
      
    ],
  },
];

export function generateSitemap() {
  function extractRoutes(items): string[] {
    return items.flatMap((item) => {
      const routes: string[] = [];

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
    outDir: "site/dist",
  });
}

export default defineConfig({
  font: {
    mono: {
      google: "Ubuntu Mono",
    },
  },
  vite: {
    plugins: [generateSitemap(), biomePlugin()],
    resolve: {
      alias: {
        "lightgallery/fonts": path.resolve(__dirname, "node_modules/lightgallery/fonts"),
        "lightgallery/images": path.resolve(__dirname, "node_modules/lightgallery/images"),
      },
    },
    server: {
      fs: {
        allow: ["node_modules/lightgallery"],
      },
    },
  },
  sidebar: SIDEBAR_CONFIG,
  topNav: [
    { text: "Indexer", link: "https://indexer.beboundless.xyz" },
    { text: "Help", link: "https://t.me/+E9J7zgtyoTVlNzk1" },
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
      link: "https://x.com/boundless_xyz",
    },
  ],
  rootDir: "site",
  title: "Boundless Docs",
  /*logoUrl: {
    light: "/logo.png",
    dark: "/logo-dark.png",
  },*/
  theme: {
    accentColor: {
      light: "#537263", // Forest - primary accent for light mode
      dark: "#AED8C4", // Leaf - lighter accent for dark mode
    },
    variables: {
      color: {
        backgroundDark: {
          light: "#EFECE3", // Sand
          dark: "#1e1d1f",
        },
        background: {
          light: "#FFFFFF",
          dark: "#232225",
        },
      },
      content: {
        width: "calc(90ch + (var(--vocs-content_horizontalPadding) * 2))",
      },
    },
  },
  iconUrl: {
    light: "/favicon.ico",
    dark: "/favicon.ico",
  },
  // banner: "Read the [Boundless Blog Article](https://risczero.com/blog/boundless-the-verifiable-compute-layer)",
  /*editLink: {
    pattern: "https://github.com/boundless-xyz/boundless/edit/main/documentation/site/pages/:path",
    text: "Edit on GitHub",
  },*/
  ogImageUrl: "https://docs.beboundless.xyz/og.png",
});
