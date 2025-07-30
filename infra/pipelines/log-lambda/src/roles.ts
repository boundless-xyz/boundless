export const accountIdToRoleName = (accountId: string) => {
  switch (accountId) {
    case '632745187633':
      return 'BoundlessProductionPowerUser';
    case '245178712747':
      return 'BoundlessStagingPowerUser';
    case '968153779208':
      return 'BoundlessOpsPowerUser';
    default:
      throw new Error(`No role name found for accountId: ${accountId}`);
  }
}