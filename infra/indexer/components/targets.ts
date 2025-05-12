import * as pulumi from "@pulumi/pulumi";

const stagingClients = [
    "0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3"
];
const stagingProvers = [
    "0x6bf69b603e9e655068e683bbffe285ea34e0f802",
    "0xf8087e8f3ba5fc4865eda2fcd3c05846982da136"
];

const prodClients = [
    "0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3",
];
const prodProvers = [
    "0x6bf69b603e9e655068e683bbffe285ea34e0f802"
];

const stackName = pulumi.getStack();
const isStaging = stackName === "staging" || stackName === "dev";

export const clients = isStaging ? stagingClients : prodClients;
export const provers = isStaging ? stagingProvers : prodProvers;
