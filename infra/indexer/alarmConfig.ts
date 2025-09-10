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
  [ChainId.BASE_SEPOLIA]: {
    [Stage.STAGING]: {
      clients: [
        {
          name: "og_offchain",
          address: "0x2624B8Bb6526CDcBAe94A25505ebc0C653B87eD8",
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
          ]
        },
        {
          name: "og_onchain",
          address: "0x2B0E9678b8db1DD44980802754beFFd89eD3c495",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_onchain",
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
          ]
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
              description: "no submitted orders in 2 hours minutes from og_offchain",
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
              description: "less than 90% success rate for 3 hour periods in 6 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 6,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            },
            {
              description: "less than 90% success rate for 8 hour periods within 10 hours from og_offchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 3600
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 8,
                datapointsToAlarm: 10,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ]
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_onchain",
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
              description: "less than 90% success rate for three consecutive hours from og_onchain",
              severity: Severity.SEV1,
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
          ]
        },
        {
          name: "signal_requestor",
          address: "0x47c76e56ad9316a5c1ab17cba87a1cc134552183",
          submissionRate: [
            {
              description: "no submitted orders in 3 hours from signal_requestor",
              severity: Severity.SEV1,
              metricConfig: {
                period: 10800
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
          successRate: [] // Signal rarely gets fulfilled on testnet.
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
          description: "greater than 20 expired orders in 60 minutes",
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
          description: "greater than 20 slashed orders in 60 minutes",
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
              description: "no submitted orders in 2 hours minutes from og_offchain",
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
              description: "less than 90% success rate for two 30 minute periods in 2 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 4,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            },
            {
              description: "less than 90% success rate for three 30 minute periods within 3 hours from og_offchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 5,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ]
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_onchain",
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
              description: "less than 90% success rate for two consecutive hours from og_onchain",
              severity: Severity.SEV1,
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
          ]
        },
        {
          name: "signal_requestor",
          address: "0x734df7809c4ef94da037449c287166d114503198",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from signal_requestor",
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
          successRate: [
            {
              description: "less than 90% success rate for two consecutive hours from signal_requestor",
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
          ]
        },
        {
          name: "kailua_og_offchain",
          address: "0x89f12aba0bcda3e708b1129eb2557b96f57b0de6",
          submissionRate: [
            {
              description: "no submitted orders in 2 hours from kailua_og_offchain",
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
              description: "no submitted orders in 30 minutes from kailua_og_offchain",
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
              description: "less than 90% success rate for two consecutive hours from kailua_og_offchain",
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
          description: "greater than 20 expired orders in 60 minutes",
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
            // Offchain orders are small orders submitted every 5 mins,
            // so we set a more aggressive success rate threshold.
            {
              description: "less than 90% success rate for two 30 minute periods in 2 hours from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 4,
                datapointsToAlarm: 2,
                comparisonOperator: "LessThanThreshold"
              }
            },
            {
              description: "less than 90% success rate for three 30 minute periods within 3 hours from og_offchain",
              severity: Severity.SEV1,
              metricConfig: {
                period: 1800
              },
              alarmConfig: {
                threshold: 0.90,
                evaluationPeriods: 5,
                datapointsToAlarm: 3,
                comparisonOperator: "LessThanThreshold"
              }
            }
          ]
        },
        {
          name: "og_onchain",
          address: "0xE198C6944Cae382902A375b0B8673084270A7f8e",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_onchain",
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
              description: "less than 90% success rate for two consecutive hours from og_onchain",
              severity: Severity.SEV1,
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
          ]
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
          description: "less than 3 fulfilled orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600
          },
          alarmConfig: {
            threshold: 3,
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
            threshold: 2,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
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
    }
  }
};
