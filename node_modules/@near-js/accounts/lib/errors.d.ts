import { ErrorObject } from 'ajv';
export declare class UnsupportedSerializationError extends Error {
    constructor(methodName: string, serializationType: string);
}
export declare class UnknownArgumentError extends Error {
    constructor(actualArgName: string, expectedArgNames: string[]);
}
export declare class ArgumentSchemaError extends Error {
    constructor(argName: string, errors: ErrorObject[]);
}
export declare class ConflictingOptions extends Error {
    constructor();
}
