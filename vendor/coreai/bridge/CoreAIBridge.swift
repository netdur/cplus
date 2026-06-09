// CoreAIBridge.swift — C ABI wrapper over Apple's Swift-first CoreAI API.
//
// This file is intentionally small and direct. The C+ package calls these
// @_cdecl functions; the Swift side owns all CoreAI value/object lifetimes.
//
// Requires an SDK that ships CoreAI.framework. Apple's coreai-models repo
// currently lists Xcode 27.0+ and macOS/iOS 27.0+ for running/app integration.

import CoreAI
import Foundation

private final class ModelBox {
    let model: AIModel

    init(_ model: AIModel) {
        self.model = model
    }
}

private final class FunctionBox {
    let function: InferenceFunction

    init(_ function: InferenceFunction) {
        self.function = function
    }
}

private final class NDArrayBox {
    var array: NDArray

    init(_ array: NDArray) {
        self.array = array
    }
}

private let errorLock = NSLock()
private var lastError = ""

private func setError(_ message: String) {
    errorLock.lock()
    lastError = message
    errorLock.unlock()
}

private func clearError() {
    setError("")
}

private func stringFromBytes(_ ptr: UnsafePointer<UInt8>?, _ len: Int) -> String? {
    guard let ptr else {
        setError("null string pointer")
        return nil
    }
    let data = Data(bytes: ptr, count: len)
    guard let value = String(data: data, encoding: .utf8) else {
        setError("invalid UTF-8 string")
        return nil
    }
    return value
}

private func retainedOpaque(_ object: AnyObject) -> UnsafeMutableRawPointer {
    return Unmanaged.passRetained(object).toOpaque()
}

private func object<T: AnyObject>(_ handle: UnsafeMutableRawPointer?, as type: T.Type) -> T? {
    guard let handle else {
        setError("null handle")
        return nil
    }
    let value = Unmanaged<AnyObject>.fromOpaque(handle).takeUnretainedValue()
    guard let typed = value as? T else {
        setError("handle has unexpected type")
        return nil
    }
    return typed
}

private func waitForAsync<T>(_ body: @escaping () async throws -> T) throws -> T {
    let semaphore = DispatchSemaphore(value: 0)
    var outcome: Result<T, Error>!

    Task {
        do {
            outcome = .success(try await body())
        } catch {
            outcome = .failure(error)
        }
        semaphore.signal()
    }

    semaphore.wait()
    return try outcome.get()
}

@_cdecl("cplus_coreai_runtime_available")
public func cplus_coreai_runtime_available() -> Int32 {
    return 1
}

@_cdecl("cplus_coreai_last_error")
public func cplus_coreai_last_error(
    _ buf: UnsafeMutablePointer<UInt8>?,
    _ len: Int
) -> Int32 {
    guard let buf, len > 0 else {
        return -1
    }

    errorLock.lock()
    let bytes = Array(lastError.utf8)
    errorLock.unlock()

    let n = min(bytes.count, len - 1)
    if n > 0 {
        for i in 0..<n {
            buf[i] = bytes[i]
        }
    }
    buf[n] = 0
    return Int32(n)
}

@_cdecl("cplus_coreai_release")
public func cplus_coreai_release(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else {
        return
    }
    Unmanaged<AnyObject>.fromOpaque(handle).release()
}

@_cdecl("cplus_coreai_model_load")
public func cplus_coreai_model_load(
    _ pathPtr: UnsafePointer<UInt8>?,
    _ pathLen: Int
) -> UnsafeMutableRawPointer? {
    clearError()
    guard let path = stringFromBytes(pathPtr, pathLen) else {
        return nil
    }

    do {
        let url = URL(fileURLWithPath: path)
        let model = try waitForAsync {
            try await AIModel(contentsOf: url)
        }
        return retainedOpaque(ModelBox(model))
    } catch {
        setError("AIModel load failed: \(error)")
        return nil
    }
}

@_cdecl("cplus_coreai_model_load_function")
public func cplus_coreai_model_load_function(
    _ modelHandle: UnsafeMutableRawPointer?,
    _ namePtr: UnsafePointer<UInt8>?,
    _ nameLen: Int
) -> UnsafeMutableRawPointer? {
    clearError()
    guard let modelBox = object(modelHandle, as: ModelBox.self),
          let name = stringFromBytes(namePtr, nameLen)
    else {
        return nil
    }

    do {
        guard let function = try modelBox.model.loadFunction(named: name) else {
            setError("function not found: \(name)")
            return nil
        }
        return retainedOpaque(FunctionBox(function))
    } catch {
        setError("loadFunction failed: \(error)")
        return nil
    }
}

@_cdecl("cplus_coreai_ndarray_create_f32")
public func cplus_coreai_ndarray_create_f32(
    _ shapePtr: UnsafePointer<UInt64>?,
    _ rank64: UInt64,
    _ dataPtr: UnsafePointer<Float>?,
    _ count64: UInt64
) -> UnsafeMutableRawPointer? {
    clearError()
    guard let shapePtr, rank64 > 0 else {
        setError("invalid shape")
        return nil
    }
    guard let dataPtr else {
        setError("null data pointer")
        return nil
    }

    let rank = Int(rank64)
    let count = Int(count64)
    let shape = (0..<rank).map { Int(shapePtr[$0]) }
    let expected = shape.reduce(1, *)
    guard expected == count else {
        setError("shape element count \(expected) does not match data count \(count)")
        return nil
    }

    // Real CoreAI API (macOS 27): build the NDArray straight from the scalar
    // buffer. `NDArray(scalars:shape:)` infers `scalarType` from the element
    // type (Float -> .float32). Filling via `view(as:)` is not an option here:
    // that accessor is `consuming`, so it would move the array out before we
    // could box it.
    let buffer = UnsafeBufferPointer(start: dataPtr, count: count)
    let array = NDArray(scalars: buffer, shape: shape)
    return retainedOpaque(NDArrayBox(array))
}

@_cdecl("cplus_coreai_function_run1_f32")
public func cplus_coreai_function_run1_f32(
    _ functionHandle: UnsafeMutableRawPointer?,
    _ inputNamePtr: UnsafePointer<UInt8>?,
    _ inputNameLen: Int,
    _ inputHandle: UnsafeMutableRawPointer?,
    _ outputNamePtr: UnsafePointer<UInt8>?,
    _ outputNameLen: Int
) -> UnsafeMutableRawPointer? {
    clearError()
    guard let functionBox = object(functionHandle, as: FunctionBox.self),
          let inputBox = object(inputHandle, as: NDArrayBox.self),
          let inputName = stringFromBytes(inputNamePtr, inputNameLen),
          let outputName = stringFromBytes(outputNamePtr, outputNameLen)
    else {
        return nil
    }

    do {
        // Real CoreAI API (macOS 27): `run` returns an `Outputs` collection; pull
        // the named output (an `InferenceValue`) and unwrap its `NDArray`. Do this
        // inside the async closure and return only the owned `NDArray`, so the
        // (potentially non-escapable) `Outputs` never crosses the closure boundary.
        let result: NDArray? = try waitForAsync {
            var outputs = try await functionBox.function.run(
                inputs: [inputName: inputBox.array]
            )
            return outputs.remove(outputName)?.ndArray
        }
        guard let output = result else {
            setError("output not found or not an NDArray: \(outputName)")
            return nil
        }
        return retainedOpaque(NDArrayBox(output))
    } catch {
        setError("inference failed: \(error)")
        return nil
    }
}

@_cdecl("cplus_coreai_ndarray_copy_f32")
public func cplus_coreai_ndarray_copy_f32(
    _ arrayHandle: UnsafeMutableRawPointer?,
    _ dest: UnsafeMutablePointer<Float>?,
    _ count64: UInt64
) -> Int64 {
    clearError()
    guard let arrayBox = object(arrayHandle, as: NDArrayBox.self),
          let dest
    else {
        return -1
    }

    let count = Int(count64)
    // Real CoreAI API (macOS 27): `view(as:)` is `consuming`, so it can't be
    // called on the boxed array in place — take a copy (`NDArray` is Copyable)
    // and consume that. The element count is the product of the view's shape.
    let tmp = arrayBox.array
    let view = tmp.view(as: Float.self)
    var elementCount = 1
    for i in 0..<view.shape.count {
        elementCount *= view.shape[i]
    }
    guard elementCount <= count else {
        setError("destination too small: \(count) < \(elementCount)")
        return -1
    }
    guard view.isContiguous else {
        setError("output view is not contiguous; strided copy is not supported in the MVP")
        return -1
    }
    let n = elementCount
    view.withUnsafePointer { ptr, _, _ in
        for i in 0..<n {
            dest[i] = ptr[i]
        }
    }
    return Int64(n)
}
