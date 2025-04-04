import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';

export class Vpc extends pulumi.ComponentResource {
  public vpcx: awsx.ec2.Vpc;
  public numberPrivateSubnets = 2;

  constructor(
    name: string,
    args: {
      /**
       * The AWS region
       */
      region: string;
      /**
       * (Optional) CIDR block for the VPC. Defaults to `10.0.0.0/16`
       */
      cidrBlock?: string;
      /**
       * Availability zones for a given region
       */
      availabilityZones: string[];
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('boundless:services:Vpc', name, {}, opts);

    let natStrategy: awsx.types.enums.ec2.NatGatewayStrategy;
    if (args.availabilityZones.length > 1) {
      natStrategy = awsx.types.enums.ec2.NatGatewayStrategy.OnePerAz;
    } else if (args.availabilityZones.length === 1) {
      natStrategy = awsx.types.enums.ec2.NatGatewayStrategy.Single;
    } else {
      throw 'Number of AZ cannot be less than 1';
    }

    if (!args.cidrBlock) {
      args.cidrBlock = '10.0.0.0/16';
    }
    const cidrLayers = args.cidrBlock.split('.');
    const baseCidr = `${cidrLayers[0]}.${cidrLayers[1]}`;

    /// Build the VPC
    this.vpcx = new awsx.ec2.Vpc(
      'base-vpc',
      {
        cidrBlock: args.cidrBlock,
        availabilityZoneNames: args.availabilityZones,
        natGateways: {
          strategy: natStrategy,
        },
        enableDnsHostnames: true,
        subnetStrategy: 'Exact',
        tags: {
          Name: 'base-vpc',
        },
        // The index in every column represents a single subnet
        // this is because cidrBlocks must follow this order:
        // - public -> private -> unused
        // Once established a subnet shouldn't be replaced.
        subnetSpecs: [
          {
            type: 'Public',
            cidrBlocks: [`${baseCidr}.0.0/19`, `${baseCidr}.64.0/19`, `${baseCidr}.128.0/19`, `${baseCidr}.192.0/19`],
          },
          {
            type: 'Private',
            cidrBlocks: [`${baseCidr}.32.0/19`, `${baseCidr}.96.0/19`, `${baseCidr}.160.0/19`, `${baseCidr}.224.0/19`],
          },
        ],
      },
      { parent: this }
    );
  }
}
