import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as fs from 'fs';

export = () => {
  const config = new pulumi.Config();
  const serviceName = config.require('SERVICE_NAME');
  const dockerDir = config.get('DOCKER_DIR') || '../../';
  const dockerTag = config.get('DOCKER_TAG') || 'latest';
  const githubTokenSecret = config.get('GH_TOKEN_SECRET');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const dockerRemoteBuilder = process.env.DOCKER_REMOTE_BUILDER;

  let buildSecrets: Record<string, pulumi.Input<string>> = {};
  if (ciCacheSecret !== undefined) {
    const cacheFileData = ciCacheSecret.apply((filePath: any) => fs.readFileSync(filePath, 'utf8'));
    buildSecrets = { ci_cache_creds: cacheFileData };
  }
  if (githubTokenSecret !== undefined) {
    buildSecrets = { ...buildSecrets, githubTokenSecret };
  }

  const repo = new awsx.ecr.Repository(`${serviceName}-images-repo`, {
    name: `${serviceName}-images`,
    forceDelete: true,
    lifecyclePolicy: {
      rules: [{
        description: 'Delete untagged images after 7 days',
        tagStatus: 'untagged' as const,
        maximumAgeLimit: 7,
      }],
    },
  });

  const authToken = aws.ecr.getAuthorizationTokenOutput({
    registryId: repo.repository.registryId,
  });

  const registries = [{
    address: repo.repository.repositoryUrl,
    password: authToken.password,
    username: authToken.userName,
  }];

  function buildImage(name: string, dockerfile: string, tag: string): docker_build.Image {
    const tagPath = pulumi.interpolate`${repo.repository.repositoryUrl}:${tag}`;
    return new docker_build.Image(`${serviceName}-${name}-image`, {
      tags: [tagPath],
      context: { location: dockerDir },
      platforms: ['linux/amd64'],
      push: true,
      secrets: buildSecrets,
      builder: dockerRemoteBuilder ? { name: dockerRemoteBuilder } : undefined,
      dockerfile: { location: `${dockerDir}/dockerfiles/${dockerfile}` },
      cacheFrom: [{
        registry: { ref: pulumi.interpolate`${repo.repository.repositoryUrl}:${name}-cache` },
      }],
      cacheTo: [{
        registry: {
          mode: docker_build.CacheMode.Max,
          imageManifest: true,
          ociMediaTypes: true,
          ref: pulumi.interpolate`${repo.repository.repositoryUrl}:${name}-cache`,
        },
      }],
      registries,
    });
  }

  const outputs: Record<string, pulumi.Output<string>> = {};

  // Primary image (every service has one)
  const dockerfile = config.require('DOCKERFILE');
  const primaryImage = buildImage('primary', dockerfile, dockerTag);
  outputs.imageRef = primaryImage.ref;

  // Optional additional images (indexer has backfill + rewards)
  const backfillDockerfile = config.get('DOCKERFILE_BACKFILL');
  if (backfillDockerfile) {
    const backfillImage = buildImage('backfill', backfillDockerfile, `backfill-${dockerTag}`);
    outputs.backfillImageRef = backfillImage.ref;
  }

  const rewardsDockerfile = config.get('DOCKERFILE_REWARDS');
  if (rewardsDockerfile) {
    const rewardsImage = buildImage('rewards', rewardsDockerfile, `rewards-${dockerTag}`);
    outputs.rewardsImageRef = rewardsImage.ref;
  }

  return outputs;
};
