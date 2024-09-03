"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ConflictingOptions = exports.ArgumentSchemaError = exports.UnknownArgumentError = exports.UnsupportedSerializationError = void 0;
class UnsupportedSerializationError extends Error {
    constructor(methodName, serializationType) {
        super(`Contract method '${methodName}' is using an unsupported serialization type ${serializationType}`);
    }
}
exports.UnsupportedSerializationError = UnsupportedSerializationError;
class UnknownArgumentError extends Error {
    constructor(actualArgName, expectedArgNames) {
        super(`Unrecognized argument '${actualArgName}', expected '${JSON.stringify(expectedArgNames)}'`);
    }
}
exports.UnknownArgumentError = UnknownArgumentError;
class ArgumentSchemaError extends Error {
    constructor(argName, errors) {
        super(`Argument '${argName}' does not conform to the specified ABI schema: '${JSON.stringify(errors)}'`);
    }
}
exports.ArgumentSchemaError = ArgumentSchemaError;
class ConflictingOptions extends Error {
    constructor() {
        super('Conflicting contract method options have been passed. You can either specify ABI or a list of view/call methods.');
    }
}
exports.ConflictingOptions = ConflictingOptions;
