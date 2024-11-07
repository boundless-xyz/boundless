import { defineConfig } from "vocs";

export default defineConfig({
  font: {
    google: "Noto Sans",
    mono: {
      google: "Ubuntu Mono",
    },
  },
  sidebar: [
    {
      text: "Market",
      collapsed: false,
      items: [
        {
          text: "Introduction",
          link: "/market",
        },
        {
          text: "Boundless Market RFC",
          link: "/market/boundless-market-rfc",
        },
        {
          text: "Market Matching Design",
          link: "/market/market-matching-design",
        },
        {
          text: "Local Development",
          link: "/market/local-development",
        },
        {
          text: "Public Deployments",
          link: "/market/public-deployments",
        },
      ],
    },
    {
      text: "Requestor Manual",
      collapsed: false,
      items: [
        {
          text: "Broadcasting Requests",
          link: "/requestor-manual/broadcasting-requests",
        },
      ],
    },
    {
      text: "Reference",
      link: "/reference",
    },
    {
      text: "Glossary",
      link: "/glossary",
    },
  ],
  topNav: [
    { text: "Guide & API", link: "/docs/getting-started", match: "/docs" },
    { text: "Blog", link: "/blog" },
    {
      text: "0.0.1",
      items: [
        {
          text: "Changelog",
          link: "https://github.com/wevm/vocs/blob/main/src/CHANGELOG.md",
        },
        {
          text: "Contributing",
          link: "https://github.com/wevm/vocs/blob/main/.github/CONTRIBUTING.md",
        },
      ],
    },
  ],
  rootDir: "site",
  title: "Boundless Docs",
});
