# Requestor Lists Infrastructure

Deploys the Boundless requestor priority lists to S3, with optional CloudFront CDN and custom domain support.

## Updating the Lists

To update the priority list:

1. Edit `../../requestor-lists/boundless-recommended-priority-list.json`
2. Merge to main

The list will be automatically refreshed by brokers according to their configured refresh interval (default: 1 hour).
