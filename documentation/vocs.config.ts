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
      items: [
        {
          text: "Broadcasting Requests",
          link: "/requestor-manual/broadcasting-requests",
        },
      ],
    },
    {
      text: "Prover Manual",
      items: [
        {
          text: "Bento",
          items: [
            {
              text: "Introduction",
              link: "/prover-manual/bento/introduction",
            },
            {
              text: "Running",
              link: "/prover-manual/bento/running",
            },
            {
              text: "Performance Tuning",
              link: "/prover-manual/bento/performance-tuning",
            },
          ],
        },
        {
          text: "Broker",
          items: [
            {
              text: "Introduction",
              link: "/prover-manual/broker/introduction",
            },
            {
              text: "Configuration",
              link: "/prover-manual/broker/configuration",
            },
            {
              text: "Operation",
              link: "/prover-manual/broker/operation",
            },
          ],
        },
        {
          text: "Monitoring",
          link: "/prover-manual/monitoring",
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
