<!--
This file contains all links - internal and external - used in the Book, and thus serves as the master reference link source for all files.

It needs to be postfixed on all pages that require use of links:

{{#include <RELATIVE PATH TO>links.md:21:}}

**NOTE: Internal links MUST be absolute paths!**
(So all links are correct from every subdir of the book)

## Internal Link Naming Convention:
  - page-... ==> internal page, no HTML id navigation
  - id-...   ==> internal page, direct HTML id navigation
  - term-... ==> glossary items (HTML id attributes)

For more info, see:
- Markdown Reference Links: https://markdownguide.offshoot.io/basic-syntax/#reference-style-links
- Including files in mdBook: https://rust-lang.github.io/mdBook/format/mdbook.html#including-files for more info
-->

<!-- External Links -->

[rapidsnark]: https://github.com/iden3/rapidsnark
[postgresql]: https://www.postgresql.org/
[docker]: https://docs.docker.com/get-started/docker-overview/
[boundless-foundry-template]: https://github.com/boundless-xyz/boundless-foundry-template/
[boundless-vision-blog]: https://risczero.com/blog/boundless-the-verifiable-compute-layer
[r0-docs-recursion]: https://dev.risczero.com/api/recursion

<!-- Market -->

[page-market-overview]: market/README.md
[page-market-rfc]: market/prover_market_rfc.md
[id-rfc-order-matching]: market/prover_market_rfc.md#order-placement-and-matching

<!-- Prover -->

[page-bento-running]: prover-manual/bento/running_bento.md
[page-bento-overview]: prover-manual/bento/README.md
[page-bento-design]: prover-manual/bento/technical_design.md
[page-broker-node]: prover-manual/broker/broker_node.md
[page-broker-devnet]: prover-manual/broker/local_devnet.md
[page-broker-sepolia]: prover-manual/broker/sepolia.md
[page-broker-overview]: prover-manual/broker/README.md

<!-- Requestor -->

[page-requestor-manual]: requestor-manual/README.md
[page-requestor-request]: requestor-manual/proving_request.md
[page-prover-manual]: prover-manual/README.mdxc
[page-glossary]: glossary.md
[page-reference]: reference.md

<!-- Glossary -->

[term-agent]: glossary.md#agent
[term-bento]: glossary.md#bento
[term-boundless]: glossary.md#boundless-market
[term-boundless-market]: glossary.md#boundless-market
[term-requestor]: glossary.md#requestor
[term-prover]: glossary.md#prover
