import { ChainId, Severity, Stage } from "../util";
import * as aws from "@pulumi/aws";

type ChainStageAlarms = {
  [C in ChainId]: {
    [S in Stage]: ChainStageAlarmConfig | undefined
  }
};

type AlarmConfig = {
  severity: Severity;
  description?: string;
  metricConfig: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric> & {
    period: number;
  };
  alarmConfig: Partial<aws.cloudwatch.MetricAlarmArgs> & {
    evaluationPeriods: number;
    datapointsToAlarm: number;
    comparisonOperator?: string;
    threshold?: number;
    treatMissingData?: string;
  };
}

type ChainStageAlarmConfig = {
  clients: {
    name: string;
    address: string;
    submissionRate: Array<AlarmConfig>;
    successRate: Array<AlarmConfig>;
    expiredRequests: Array<AlarmConfig>;
  }[];
  provers: Array<{
    name: string;
    address: string;
  }>;
  topLevel: {
    fulfilledRequests: Array<AlarmConfig>;
    submittedRequests: Array<AlarmConfig>;
    expiredRequests: Array<AlarmConfig>;
    slashedRequests: Array<AlarmConfig>;
  }
};

export const alarmConfig: ChainStageAlarms = {
  [ChainId.ETH_MAINNET]: {
    [Stage.STAGING]: undefined,
    [Stage.PROD]: undefined,
  },
  [ChainId.BASE_SEPOLIA]: {
    [Stage.STAGING]: {
      clients: [
        {
          name: "og_offchain",
          address: "0x2624B8Bb6526CDcBAe94A25505ebc0C653B87eD8",
          submissionRate: [
            {
              description: "no submitted orders for two consecutive hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 2,
                datapointsToAlarm: 2,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              // Since we deploy with CI to staging, and this causes all the provers to restart,
              // which can take a long time, especially if multiple changes are pushed subsequently.
              // We set a longer time period for the success rate.
              description: "less than 90% success rate for three consecutive hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 3,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: [{
            description: "greater than or equal to 3 expired orders for two consecutive hours from og_offchain",
            severity: Severity.SEV2,
            metricConfig: {
              period: 3600,
            },
            alarmConfig: {
              threshold: 3,
              evaluationPeriods: 2,
              datapointsToAlarm: 2,
              comparisonOperator: "GreaterThanOrEqualToThreshold",
            }
          }],
        },
        {
          name: "kailua_order_generator",
          address: "0xf353bDA16a83399C11E09615Ee7ac326a5a08Ccf",
          submissionRate: [
            {
              description: "no submitted orders for two consecutive hours from kailua_order_generator",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 2,
                datapointsToAlarm: 2,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              description: "less than 90% success rate for two consecutive hours from kailua_order_generator",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 2,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "og_onchain",
          address: "0x2B0E9678b8db1DD44980802754beFFd89eD3c495",
          submissionRate: [
            {
              description: "no submitted orders for two consecutive hours from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 2,
                datapointsToAlarm: 2,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              // Since we deploy with CI to staging, and this causes all the provers to restart,
              // which can take a long time, especially if multiple changes are pushed subsequently.
              // We set a longer time period for the success rate.
              description: "less than 90% success rate for three consecutive hours from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 3,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: [{
            description: "greater than or equal to 3 expired orders for two consecutive hours from og_onchain",
            severity: Severity.SEV2,
            metricConfig: {
              period: 3600,
            },
            alarmConfig: {
              threshold: 3,
              evaluationPeriods: 2,
              datapointsToAlarm: 2,
              comparisonOperator: "GreaterThanOrEqualToThreshold",
            }
          }]
        }
      ],
      provers: [
        {
          name: "boundless-bento-1",
          address: "0x17bFC5a095B1F76dc8DADC6BC237E8473082D3b2"
        },
        {
          name: "boundless-bento-2",
          address: "0x55C0615B1B87054072434f277b72bB85ceF173C9"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders for two consecutive hours",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        submittedRequests: [{
          description: "less than 2 submitted orders in 30 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 1800
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        // Expired and slashed requests are not necessarily problems with the market. We keep these at low threshold
        // just during the initial launch for monitoring purposes.
        expiredRequests: [{
          description: "greater than 15 expired orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 15,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }],
        slashedRequests: [{
          description: "greater than 15 slashed orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 15,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }]
      }
    },
    [Stage.PROD]: {
      clients: [
        {
          name: "og_offchain",
          address: "0xc197eBE12C7Bcf1d9F3b415342bDbC795425335C",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              description: "less than 50% success rate for 3 hour periods in 6 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 6,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 30 minutes from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              description: "less than 50% success rate for three consecutive hours from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 3,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "signal_requestor",
          address: "0x47c76e56ad9316a5c1ab17cba87a1cc134552183",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from signal_requestor",
              severity: Severity.SEV2,
              metricConfig: {
                period: 7200
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [], // Signal rarely gets fulfilled on testnet.
          expiredRequests: []
        }
      ],
      provers: [
        {
          name: "r0-bento-1",
          address: "0xade5C4b00Ab283608928c29e55917899DA8aC608"
        },
        {
          name: "r0-bento-prod-coreweave",
          address: "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
        },
        {
          name: "r0-bento-2",
          address: "0x15a9A6A719c89Ecfd7fCa1893b975D68aB2D77A9"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        submittedRequests: [{
          description: "less than 2 submitted orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        // Expired and slashed requests are not necessarily problems with the market. We keep these at low threshold
        // just during the initial launch for monitoring purposes.
        expiredRequests: [{
          description: "greater than 50 expired orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 20,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }],
        slashedRequests: [{
          description: "greater than 50 slashed orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 20,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }]
      }
    }
  },
  [ChainId.BASE]: {
    [Stage.STAGING]: undefined, // No staging env for Base mainnet.
    [Stage.PROD]: {
      clients: [
        {
          name: "og_offchain",
          address: "0xc197eBE12C7Bcf1d9F3b415342bDbC795425335C",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from og_offchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 7200
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            },
            {
              description: "no submitted orders in 60 minutes from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            {
              // Since current submit every 5 mins, this is >= 2 failures an hour
              description: "less than 50% success rate for two 30 minute periods in 2 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 4,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            },
            {
              description: "less than 50% success rate for six 30 minute periods within 4 hours from og_offchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 8,
                datapointsToAlarm: 6,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from og_onchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 7200
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            },
            {
              description: "no submitted orders in 30 minutes from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            // Onchain orders are large orders that can take variable lengths of time to fulfill,
            // so we set a more lenient success rate threshold, since there may be periods where
            // fewer proofs get fulfilled due to variant proof lengths.
            {
              description: "less than 50% success rate for two consecutive hours from og_onchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 2,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "signal_requestor",
          address: "0x734df7809c4ef94da037449c287166d114503198",
          submissionRate: [
            {
              description: "no submitted orders in 1 hour from signal_requestor",
              severity: Severity.SEV1,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            },
            {
              description: "no submitted orders in 30 minutes from signal_requestor",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [],
          expiredRequests: [{
            description: "greater than or equal to 1 expired orders across 2 hours from signal_requestor",
            severity: Severity.SEV2,
            metricConfig: {
              period: 3600,
            },
            alarmConfig: {
              threshold: 1,
              evaluationPeriods: 2,
              datapointsToAlarm: 2,
              comparisonOperator: "GreaterThanOrEqualToThreshold",
            }
          }],
        },
        {
          name: "kailua_og_offchain_2",
          address: "0x89f12aba0bcda3e708b1129eb2557b96f57b0de6",
          submissionRate: [
            // {
            //   description: "no submitted orders in 2 hours from kailua_og_offchain",
            //   severity: Severity.SEV2,
            //   metricConfig: {
            //     period: 7200
            //   },
            //   alarmConfig: {
            //     evaluationPeriods: 1,
            //     datapointsToAlarm: 1,
            //     threshold: 1,
            //     comparisonOperator: "LessThanThreshold",
            //     treatMissingData: "breaching"
            //   }
            // },
            // {
            //   description: "no submitted orders in 30 minutes from kailua_og_offchain",
            //   severity: Severity.SEV2,
            //   metricConfig: {
            //     period: 1800
            //   },
            //   alarmConfig: {
            //     evaluationPeriods: 1,
            //     datapointsToAlarm: 1,
            //     threshold: 1,
            //     comparisonOperator: "LessThanThreshold",
            //     treatMissingData: "breaching"
            //   }
            // }
          ],
          successRate: [
            // {
            //   description: "less than 90% success rate for two consecutive hours from kailua_og_offchain",
            //   severity: Severity.SEV2,
            //   metricConfig: {
            //     period: 3600
            //   },
            //   alarmConfig: {
            //     threshold: 0.90,
            //     evaluationPeriods: 2,
            //     datapointsToAlarm: 2,
            //     comparisonOperator: "LessThanThreshold"
            //   }
            // }
          ],
          expiredRequests: []
        },
        {
          name: "cranberries",
          address: "0xc4ce4f04b9907a9401a0ed7ef073dffebab52aab",
          submissionRate: [],
          successRate: [],
          expiredRequests: [{
            description: "greater than or equal to 2 expired orders over 6 hours from cranberries",
            severity: Severity.SEV2,
            metricConfig: {
              period: 21600,
            },
            alarmConfig: {
              threshold: 2,
              evaluationPeriods: 1,
              datapointsToAlarm: 1,
              comparisonOperator: "GreaterThanOrEqualToThreshold",
            }
          }],
        },
        {
          name: "Marionberry",
          address: "0x323ec32ef13716bfdc3e0b2a96d7bd7cbcb9d57b",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Marionberry",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Lingonberry #2",
          address: "0x2D611BE1e2E49C7b639A88b507b532DaE35e492b",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Lingonberry #2",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Lingonberry",
          address: "0xb59bb74fa0c1611eF6A4989a92C0d3ca6942fC0c",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Lingonberry",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Dewberries #1",
          address: "0xf23ce39Bf3Ea20C7736C0BBe835B60671EC0b4b2",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Dewberries #1",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Dewberries #2",
          address: "0xE3838098D694f572d9eEA90f3b7A65ec3864D568",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Dewberries #2",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Strawberry",
          address: "0xb686f080c2c7045d13d196787c99bc9972480fd3",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Strawberry",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Acai",
          address: "0x418be9167e835ad820df53fd16b8fe02f796ce1c",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Acai",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Tomatillo",
          address: "0x382bba7d7bc9ae86c5de3e16c4ca96bcc0a3478e",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Tomatillo",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "Barberry",
          address: "0x076aca5883adb08c31e9308c59cd8495a2513da7",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from Barberry",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        },
        {
          name: "lingonberry test",
          address: "0x1578934C9B006B9568D884C421704d996D402B78",
          submissionRate: [],
          successRate: [],
          expiredRequests: [
            {
              description: "greater than or equal to 2 expired orders over 6 hours from lingonberry test",
              severity: Severity.SEV2,
              metricConfig: {
                period: 21600,
              },
              alarmConfig: {
                threshold: 2,
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                comparisonOperator: "GreaterThanOrEqualToThreshold",
              }
            }
          ]
        }
      ],
      provers: [
        {
          name: "r0-bonsai-1",
          address: "0xade5C4b00Ab283608928c29e55917899DA8aC608"
        },
        {
          name: "r0-bento-prod-coreweave",
          address: "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
        },
        {
          name: "r0-bento-2",
          address: "0x15a9A6A719c89Ecfd7fCa1893b975D68aB2D77A9"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        },
        {
          description: "less than 1 fulfilled orders in 60 minutes",
          severity: Severity.SEV1,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 1,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        submittedRequests: [{
          description: "less than 2 submitted orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        },
        {
          description: "less than 1 submitted orders in 30 minutes",
          severity: Severity.SEV1,
          metricConfig: {
            period: 1800,
          },
          alarmConfig: {
            threshold: 1,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        // Expired and slashed requests are not necessarily problems with the market. We keep these at low threshold
        // just during the initial launch for monitoring purposes.
        expiredRequests: [{
          description: "greater than 50 expired orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 50,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }],
        slashedRequests: [{
          description: "greater than 50 slashed orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 50,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }]
      }
    }
  },
  [ChainId.ETH_SEPOLIA]: {
    [Stage.STAGING]: undefined,
    [Stage.PROD]: {
      clients: [
        {
          name: "og_offchain",
          address: "0xc197eBE12C7Bcf1d9F3b415342bDbC795425335C",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            // Offchain orders are small orders submitted every 5 mins,
            // so we set a more aggressive success rate threshold.
            {
              description: "less than 50% success rate for two 30 minute periods in 2 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 4,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 7200
              },
              alarmConfig: {
                evaluationPeriods: 1,
                datapointsToAlarm: 1,
                threshold: 1,
                comparisonOperator: "LessThanThreshold",
                treatMissingData: "breaching"
              }
            }
          ],
          successRate: [
            // Onchain orders are large orders that can take variable lengths of time to fulfill,
            // so we set a more lenient success rate threshold, since there may be periods where
            // fewer proofs get fulfilled due to variant proof lengths.
            {
              description: "less than 50% success rate for four consecutive hours from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.50,
                evaluationPeriods: 4,
                datapointsToAlarm: 4,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ],
          expiredRequests: []
        }
      ],
      provers: [
        {
          name: "r0-bento-1",
          address: "0xade5C4b00Ab283608928c29e55917899DA8aC608"
        },
        {
          name: "r0-bento-prod-coreweave",
          address: "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
        },
        {
          name: "r0-bento-2",
          address: "0x15a9A6A719c89Ecfd7fCa1893b975D68aB2D77A9"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        submittedRequests: [{
          description: "less than 2 submitted orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching"
          }
        }],
        // Expired and slashed requests are not necessarily problems with the market. We keep these at low threshold
        // just during the initial launch for monitoring purposes.
        expiredRequests: [{
          description: "greater than 50 expired orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 50,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }],
        slashedRequests: [{
          description: "greater than 50 slashed orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 50,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }]
      }
    }
  }
};
