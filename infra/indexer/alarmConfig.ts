import { ChainId, Severity, Stage } from "../util";
import * as aws from "@pulumi/aws";

type ChainStageAlarms = {
  [C in ChainId]: {
    [S in Stage]: ChainStageAlarmConfig
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
  [ChainId.BASE]: {
    [Stage.STAGING]: {
      clients: [
        {
          name: "og_offchain",
          address: "0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3",
          submissionRate: [
            {
              description: "no submitted orders in 30 minutes from og_offchain",
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
              // Since current submit every 5 mins, this is >= 2 failures an hour
              // Since we deploy with CI to staging, and this causes all the provers to restart,
              // we set a longer time period for the success rate. If we see two consecutive hours
              // with less than 90% success rate, we will trigger an alarm.
              description: "less than 90% success rate for two consecutive hours for og_offchain",
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
          name: "og_onchain",
          address: "0x8934790e351cbcadd11fc6f9729257cd64f860bf",
          submissionRate: [
            {
              description: "no submitted orders in 15 minutes from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 900
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
              // we set a longer time period for the success rate.
              description: "less than 90% success rate for two consecutive hours from og_onchain",
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
          name: "r0-bonsai-staging",
          address: "0x6bf69b603e9e655068e683bbffe285ea34e0f802"
        },
        {
          name: "r0-bento-staging",
          address: "0xd51001491dF1c653d3ef8017Cc9f8B5282FD81FB"
        }
      ],
      topLevel: {
        // Since we deploy with CI to staging, and this causes all the provers to restart,
        // we set a longer time period for the success rate.
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 30 minutes",
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
          description: "greater than 5 expired orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 5,
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
          }
        }],
        slashedRequests: [{
          description: "greater than 5 slashed orders in 60 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 3600,
          },
          alarmConfig: {
            threshold: 5,
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
          address: "0x2546c553d857d20658ece248f7c7d0861a240681",
          submissionRate: [
            {
              description: "no submitted orders in 30 minutes from og_offchain",
              severity: Severity.SEV1,
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
            },
            {
              description: "no submitted orders in 15 minutes from og_offchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 900
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
          address: "0xc2db89b2bd434ceac6c74fbc0b2ad3a280e66db0",
          submissionRate: [
            {
              description: "no submitted orders in 30 minutes from og_onchain",
              severity: Severity.SEV1,
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
            },
            {
              description: "no submitted orders in 15 minutes from og_onchain",
              severity: Severity.SEV2,
              metricConfig: {
                period: 900
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
          name: "r0-bonsai-prod",
          address: "0x3da7206e104f6d5dd070bfe06c5373cc45c3e65c"
        },
        {
          name: "r0-bento-prod-coreweave",
          address: "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
        },
        {
          name: "r0-bento-prod",
          address: "0xbdA9Dd4b984b3f0b62A167f965c5fDC18EED5542"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 10 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 600
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
          description: "less than 1 fulfilled orders in 15 minutes",
          severity: Severity.SEV1,
          metricConfig: {
            period: 900
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
          description: "less than 1 submitted orders in 15 minutes",
          severity: Severity.SEV1,
          metricConfig: {
            period: 900,
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
  [ChainId.SEPOLIA]: {
    [Stage.STAGING]: {
      clients: [
        {
          name: "og_offchain",
          address: "0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3",
          submissionRate: [
            {
              description: "no submitted orders in 30 minutes from og_offchain",
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
              // Since we deploy with CI to staging, and this causes all the provers to restart,
              // we set a longer time period for the success rate.
              description: "less than 90% success rate for two consecutive hours from og_offchain",
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
          name: "og_onchain",
          address: "0x8934790e351cbcadd11fc6f9729257cd64f860bf",
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
              // Since current submit every 5 mins, this is >= 2 failures an hour
              // Since we deploy with CI to staging, and this causes all the provers to restart,
              // we set a longer time period for the success rate.
              description: "less than 90% success rate for two consecutive hours from og_onchain",
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
          name: "r0-bonsai-staging",
          address: "0x6bf69b603e9e655068e683bbffe285ea34e0f802"
        },
        {
          name: "r0-bento-staging",
          address: "0xd51001491dF1c653d3ef8017Cc9f8B5282FD81FB"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 2 fulfilled orders in 30 minutes",
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
          address: "0x2546c553d857d20658ece248f7c7d0861a240681",
          submissionRate: [
            {
              description: "no submitted orders in 60 minutes from og_offchain",
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
              description: "no submitted orders in 30 minutes from og_offchain",
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
          address: "0xc2db89b2bd434ceac6c74fbc0b2ad3a280e66db0",
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
          name: "r0-bonsai-prod",
          address: "0x3da7206e104f6d5dd070bfe06c5373cc45c3e65c"
        },
        {
          name: "r0-bento-prod-coreweave",
          address: "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
        },
        {
          name: "r0-bento-prod",
          address: "0xbdA9Dd4b984b3f0b62A167f965c5fDC18EED5542"
        }
      ],
      topLevel: {
        fulfilledRequests: [{
          description: "less than 3 fulfilled orders in 30 minutes",
          severity: Severity.SEV2,
          metricConfig: {
            period: 1800
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
          description: "less than 2 fulfilled orders in 30 minutes",
          severity: Severity.SEV1,
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