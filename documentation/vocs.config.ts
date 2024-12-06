import biomePlugin from "vite-plugin-biome";
import VitePluginSitemap from "vite-plugin-sitemap";
import { defineConfig } from "vocs";

const SIDEBAR_CONFIG = [
  {
    text: "✨ Introduction",
    collapsed: false,
    items: [
      {
        text: "Why Boundless?",
        link: "/introduction/why-boundless",
      },
      {
        text: "What is Boundless?",
        link: "/introduction/what-is-boundless",
      },
      {
        text: "Proof Lifecycle",
        link: "/introduction/proof-lifecycle",
      },
    ],
  },
  {
    text: "🏋️ Build",
    collapsed: false,
    items: [
      {
        text: "Build a Program",
        link: "/build/build-a-program",
      },
      {
        text: "Request a Proof",
        link: "/build/request-a-proof",
      },
      {
        text: "Use a Proof",
        link: "/build/use-a-proof",
      },
    ],
  },
  {
    text: "🧠 Advanced & References",
    collapsed: false,
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
  },
  sidebar: SIDEBAR_CONFIG,
  topNav: [
    { text: "Indexer", link: "https://indexer.beboundless.xyz" },
    {
      text: process.env.LATEST_TAG || "Latest",
      items: [
        {
          text: "Releases",
          link: "https://github.com/boundless-xyz/boundless/releases",
        },
      ],
    },
  ],
  socials: [
    {
      icon: "github",
      link: "https://github.com/boundless-xyz",
    },
    {
      icon: "x",
      link: "https://x.com/boundless_xyz",
    },
  ],
  rootDir: "site",
  title: "Boundless Documentation",
  logoUrl: {
    light: "/logo.png",
    dark: "/logo-dark.png",
  },
  theme: {
    accentColor: {
      light: "#537263", // Forest - primary accent
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
    light: "/favicon.svg",
    dark: "/favicon-dark.svg",
  },
  // banner: "Read the [Boundless Blog Article](https://risczero.com/blog/boundless-the-verifiable-compute-layer)",
  editLink: {
    pattern: "https://github.com/boundless-xyz/boundless/edit/main/documentation/site/pages/:path",
    text: "Edit on GitHub",
  },
  ogImageUrl:
    "https://vocs.dev/api/og?logo=https://boundless-documentation.vercel.app/logo-dark.png&title=%title&description=%description",
});
