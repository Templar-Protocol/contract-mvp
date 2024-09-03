import { JSONSchema7 } from 'json-schema';
export declare const SCHEMA_VERSION = "0.3.0";
/** Root model representing the entire contract ABI with all its functions and custom types. */
export interface AbiRoot {
    /** Semver of the ABI schema format. */
    schema_version: string;
    /** Metadata information about the contract. */
    metadata: AbiMetadata;
    /** Core ABI information (functions and types). */
    body: AbiBody;
}
/** Metadata information about the contract. */
export interface AbiMetadata {
    /** The name of the smart contract. */
    name?: string;
    /** The version of the smart contract. */
    version?: string;
    /** The authors of the smart contract. */
    authors?: string[];
    /** The information about how this contract was built. */
    build?: BuildInfo;
    /** The SHA-256 hash of the contract WASM code in Base58 format. */
    wasm_hash?: string;
}
export interface BuildInfo {
    /** The compiler (versioned) that was used to build the contract. */
    compiler: string;
    /** The build tool (versioned) that was used to build the contract. */
    builder: string;
    /** The docker image (versioned) where the contract was built. */
    image?: string;
}
/** Core ABI information. */
export interface AbiBody {
    /** ABIs of all contract's functions. */
    functions: AbiFunction[];
    /** Root JSON schema for the ABI. */
    root_schema: JSONSchema7;
}
/** ABI of a single function. */
export interface AbiFunction {
    name: string;
    /** Human-readable documentation parsed from the source file. */
    doc?: string;
    /** Function kind that regulates whether the function has to be invoked from a transaction. */
    kind: AbiFunctionKind;
    /** List of modifiers affecting the function. */
    modifiers?: AbiFunctionModifier[];
    /** Type identifiers of the function parameters. */
    params?: AbiParameters;
    /** Type identifiers of the callbacks of the function. */
    callbacks?: AbiType[];
    /** Type identifier of the vararg callbacks of the function. */
    callbacks_vec?: AbiType;
    /** Return type identifier. */
    result?: AbiType;
}
/**
 * Function kind regulates whether this function's invocation requires a transaction (so-called
 * call functions) or not (view functions).
 */
export declare enum AbiFunctionKind {
    View = "view",
    Call = "call"
}
/** Function can have multiple modifiers that can change its semantics. */
export declare enum AbiFunctionModifier {
    /** Init functions can be used to initialize the state of the contract. */
    Init = "init",
    /**
     * Private functions can only be called from the contract containing them. Usually, when a
     * contract has to have a callback for a remote cross-contract call, this callback method
     * should only be called by the contract itself.
     */
    Private = "private",
    /** Payable functions can accept token transfer together with the function call.
     * This is done so that contracts can define a fee in tokens that needs to be payed when
     * they are used.
     */
    Payable = "payable"
}
/** Supported serialization formats. */
export declare enum AbiSerializationType {
    Json = "json",
    Borsh = "borsh"
}
/** A list of function parameters sharing the same serialization type. */
export declare type AbiParameters = {
    args: AbiJsonParameter[];
    serialization_type: AbiSerializationType.Json;
} | {
    args: AbiBorshParameter[];
    serialization_type: AbiSerializationType.Borsh;
};
/** Information about a single named JSON function parameter. */
export interface AbiJsonParameter {
    /** Parameter name (e.g. `p1` in `fn foo(p1: u32) {}`). */
    name: string;
    /**
     * JSON Subschema that represents this type (can be an inline primitive, a reference to the root
     * schema and a few other corner-case things).
     */
    type_schema: JSONSchema7;
}
/** Information about a single named Borsh function parameter. */
export interface AbiBorshParameter {
    /** Parameter name (e.g. `p1` in `fn foo(p1: u32) {}`). */
    name: string;
    /** Inline Borsh schema that represents this type. */
    type_schema: any;
}
/** Information about a single type (e.g. function return type). */
export interface AbiType {
    /** The serialization format to be used for values of this type. */
    serialization_type: AbiSerializationType;
    /** Schema describing the type (the actual schema format depends on `serialization_type`). */
    type_schema: any;
}
/** ABI type which values are serialized using JSON format. */
export interface AbiJsonType extends AbiType {
    serialization_type: AbiSerializationType.Json;
    /** JSON Subschema that represents this type (can be an inline primitive, a reference to the root schema and a few other corner-case things). */
    type_schema: JSONSchema7;
}
/** ABI type which values are serialized using Borsh format. */
export interface AbiBorshType extends AbiType {
    serialization_type: AbiSerializationType.Borsh;
    /** Borsh schema that represents this type. */
    type_schema: any;
}
