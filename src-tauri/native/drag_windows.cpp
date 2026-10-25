#define NOMINMAX
#include <windows.h>
#include <ole2.h>
#include <shlobj.h>
#include <shldisp.h>
#include <wrl/client.h>
#include <atomic>
#include <algorithm>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <vector>
#include "drag.h"
using Microsoft::WRL::ComPtr;

static std::wstring wide(const char *s) {
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, s, -1, nullptr, 0);
    if (!n) return {};
    std::wstring out(n, L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, s, -1, out.data(), n);
    out.pop_back(); return out;
}
// Free-threaded marshaling keeps slow Read/Stat calls on the receiver's worker,
// rather than marshaling them back onto Tauri's UI STA.
class FileStream final : public IStream {
    std::atomic<ULONG> refs{1};
    void *ctx; ImmichCallbacks cb; DWORD uiThread; uint64_t position;
    std::wstring name; std::mutex gate; ComPtr<IUnknown> marshaler;
public:
    FileStream(void *c, ImmichCallbacks f, DWORD ui, std::wstring n, uint64_t pos = 0)
        : ctx(f.retain(c)), cb(f), uiThread(ui), position(pos), name(std::move(n)) {
        CoCreateFreeThreadedMarshaler(this, &marshaler);
    }
    ~FileStream() { cb.release(ctx); }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id, void **out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (id == IID_IUnknown || id == IID_IStream || id == IID_ISequentialStream) { *out = static_cast<IStream *>(this); AddRef(); return S_OK; }
        return id == IID_IMarshal && marshaler ? marshaler->QueryInterface(id, out) : E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return ++refs; }
    ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs; if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE Read(void *buffer, ULONG count, ULONG *read) override {
        if (read) *read = 0;
        if (!buffer && count) return STG_E_INVALIDPOINTER;
        if (GetCurrentThreadId() == uiThread) return E_PENDING;
        std::lock_guard<std::mutex> lock(gate);
        size_t total = 0;
        while (total < count) {
            size_t n = 0, wanted = std::min<size_t>(count - total, 64 * 1024);
            if (!cb.read_file(ctx, position, static_cast<unsigned char *>(buffer) + total, wanted, &n)) return STG_E_READFAULT;
            if (!n) break;
            total += n; position += n;
        }
        if (read) *read = static_cast<ULONG>(total);
        return total == count ? S_OK : S_FALSE;
    }
    HRESULT STDMETHODCALLTYPE Write(const void *, ULONG, ULONG *) override { return STG_E_ACCESSDENIED; }
    HRESULT STDMETHODCALLTYPE Seek(LARGE_INTEGER move, DWORD origin, ULARGE_INTEGER *next) override {
        std::lock_guard<std::mutex> lock(gate);
        uint64_t base = 0;
        if (origin == STREAM_SEEK_CUR) base = position;
        else if (origin == STREAM_SEEK_END) {
            if (GetCurrentThreadId() == uiThread) return E_PENDING;
            if (!cb.file_size(ctx, &base)) return STG_E_READFAULT;
        } else if (origin != STREAM_SEEK_SET) return STG_E_INVALIDFUNCTION;
        if ((move.QuadPart < 0 && uint64_t(-(move.QuadPart + 1)) + 1 > base)
            || (move.QuadPart >= 0 && uint64_t(move.QuadPart) > UINT64_MAX - base)) return STG_E_INVALIDFUNCTION;
        position = base + move.QuadPart;
        if (next) next->QuadPart = position;
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE SetSize(ULARGE_INTEGER) override { return STG_E_ACCESSDENIED; }
    HRESULT STDMETHODCALLTYPE CopyTo(IStream *target, ULARGE_INTEGER count, ULARGE_INTEGER *read, ULARGE_INTEGER *written) override {
        if (!target) return E_POINTER;
        if (read) read->QuadPart = 0; if (written) written->QuadPart = 0;
        unsigned char buffer[64 * 1024]; uint64_t total = 0;
        while (total < count.QuadPart) {
            ULONG n = 0, put = 0;
            HRESULT hr = Read(buffer, static_cast<ULONG>(std::min<uint64_t>(sizeof(buffer), count.QuadPart - total)), &n);
            if (FAILED(hr)) return hr;
            if (!n) break;
            hr = target->Write(buffer, n, &put);
            total += n; if (read) read->QuadPart = total; if (written) written->QuadPart += put;
            if (FAILED(hr)) return hr; if (put != n) return STG_E_WRITEFAULT;
        }
        return total == count.QuadPart ? S_OK : S_FALSE;
    }
    HRESULT STDMETHODCALLTYPE Commit(DWORD) override { return S_OK; }
    HRESULT STDMETHODCALLTYPE Revert() override { return STG_E_INVALIDFUNCTION; }
    HRESULT STDMETHODCALLTYPE LockRegion(ULARGE_INTEGER, ULARGE_INTEGER, DWORD) override { return STG_E_INVALIDFUNCTION; }
    HRESULT STDMETHODCALLTYPE UnlockRegion(ULARGE_INTEGER, ULARGE_INTEGER, DWORD) override { return STG_E_INVALIDFUNCTION; }
    HRESULT STDMETHODCALLTYPE Stat(STATSTG *stat, DWORD flags) override {
        if (!stat) return E_POINTER; *stat = {};
        if (GetCurrentThreadId() == uiThread) return E_PENDING;
        uint64_t size; if (!cb.file_size(ctx, &size)) return STG_E_READFAULT;
        stat->type = STGTY_STREAM; stat->cbSize.QuadPart = size; stat->grfMode = STGM_READ;
        if (!(flags & STATFLAG_NONAME)) {
            stat->pwcsName = static_cast<wchar_t *>(CoTaskMemAlloc((name.size() + 1) * sizeof(wchar_t)));
            if (!stat->pwcsName) return E_OUTOFMEMORY;
            std::memcpy(stat->pwcsName, name.c_str(), (name.size() + 1) * sizeof(wchar_t));
        }
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE Clone(IStream **out) override {
        if (!out) return E_POINTER; std::lock_guard<std::mutex> lock(gate);
        *out = new FileStream(ctx, cb, uiThread, name, position); return S_OK;
    }
};

class FileData final : public IDataObject, public IDataObjectAsyncCapability {
    std::atomic<ULONG> refs{1}; std::atomic<bool> asyncMode{true}, inOperation{false};
    void *ctx; ImmichCallbacks cb; std::wstring name; DWORD uiThread;
    CLIPFORMAT descriptor, contents, effect;
    ComPtr<IUnknown> marshaler; std::mutex gate;
    std::map<CLIPFORMAT, std::vector<unsigned char>> extra;
    HRESULT memory(const void *bytes, size_t len, STGMEDIUM *out) {
        HGLOBAL memory = GlobalAlloc(GMEM_MOVEABLE, len); if (!memory) return E_OUTOFMEMORY;
        void *ptr = GlobalLock(memory); if (!ptr) { GlobalFree(memory); return E_OUTOFMEMORY; }
        std::memcpy(ptr, bytes, len); GlobalUnlock(memory);
        out->tymed = TYMED_HGLOBAL; out->hGlobal = memory; out->pUnkForRelease = nullptr; return S_OK;
    }
public:
    FileData(void *c, ImmichCallbacks f, std::wstring n) : ctx(c), cb(f), name(std::move(n)), uiThread(GetCurrentThreadId()) {
        descriptor = RegisterClipboardFormatW(L"FileGroupDescriptorW"); contents = RegisterClipboardFormatW(L"FileContents");
        effect = RegisterClipboardFormatW(L"Preferred DropEffect");
        CoCreateFreeThreadedMarshaler(static_cast<IDataObject *>(this), &marshaler);
    }
    ~FileData() { cb.release(ctx); }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id, void **out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (id == IID_IUnknown || id == IID_IDataObject) *out = static_cast<IDataObject *>(this);
        else if (id == IID_IDataObjectAsyncCapability) *out = static_cast<IDataObjectAsyncCapability *>(this);
        else return id == IID_IMarshal && marshaler ? marshaler->QueryInterface(id, out) : E_NOINTERFACE;
        AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return ++refs; }
    ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs; if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE GetData(FORMATETC *fmt, STGMEDIUM *out) override {
        if (!fmt || !out) return E_POINTER; *out = {};
        HRESULT hr = QueryGetData(fmt); if (FAILED(hr)) return hr;
        if (fmt->cfFormat == contents) {
            out->tymed = TYMED_ISTREAM; out->pstm = new FileStream(ctx, cb, uiThread, name); return S_OK;
        }
        if (fmt->cfFormat == descriptor) {
            FILEGROUPDESCRIPTORW group{}; group.cItems = 1;
            auto &file = group.fgd[0]; file.dwFlags = FD_ATTRIBUTES | FD_PROGRESSUI; file.dwFileAttributes = FILE_ATTRIBUTE_NORMAL;
            wcsncpy_s(file.cFileName, name.c_str(), _TRUNCATE);
            return memory(&group, sizeof(group), out);
        }
        if (fmt->cfFormat == effect) { DWORD copy = DROPEFFECT_COPY; return memory(&copy, sizeof(copy), out); }
        std::lock_guard<std::mutex> lock(gate); auto it = extra.find(fmt->cfFormat);
        return it == extra.end() ? DV_E_FORMATETC : memory(it->second.data(), it->second.size(), out);
    }
    HRESULT STDMETHODCALLTYPE GetDataHere(FORMATETC *, STGMEDIUM *) override { return DATA_E_FORMATETC; }
    HRESULT STDMETHODCALLTYPE QueryGetData(FORMATETC *fmt) override {
        if (!fmt) return E_POINTER; if (fmt->dwAspect != DVASPECT_CONTENT) return DV_E_DVASPECT;
        if (fmt->cfFormat == contents) return fmt->lindex != 0 ? DV_E_LINDEX : ((fmt->tymed & TYMED_ISTREAM) ? S_OK : DV_E_TYMED);
        if (!(fmt->tymed & TYMED_HGLOBAL)) return DV_E_TYMED;
        if (fmt->cfFormat == descriptor || fmt->cfFormat == effect) return S_OK;
        std::lock_guard<std::mutex> lock(gate); return extra.count(fmt->cfFormat) ? S_OK : DV_E_FORMATETC;
    }
    HRESULT STDMETHODCALLTYPE GetCanonicalFormatEtc(FORMATETC *, FORMATETC *out) override {
        if (out) out->ptd = nullptr; return DATA_S_SAMEFORMATETC;
    }
    HRESULT STDMETHODCALLTYPE SetData(FORMATETC *fmt, STGMEDIUM *value, BOOL take) override {
        if (!fmt || !value) return E_POINTER;
        if (value->tymed != TYMED_HGLOBAL) return DV_E_TYMED;
        SIZE_T size = GlobalSize(value->hGlobal); if (!size || size > 1024 * 1024) return E_INVALIDARG;
        void *bytes = GlobalLock(value->hGlobal); if (!bytes) return E_OUTOFMEMORY;
        { std::lock_guard<std::mutex> lock(gate); auto *p = static_cast<unsigned char *>(bytes); extra[fmt->cfFormat] = std::vector<unsigned char>(p, p + size); }
        GlobalUnlock(value->hGlobal); if (take) ReleaseStgMedium(value); return S_OK;
    }
    HRESULT STDMETHODCALLTYPE EnumFormatEtc(DWORD direction, IEnumFORMATETC **out) override {
        if (direction != DATADIR_GET) return E_NOTIMPL;
        FORMATETC formats[] = {{descriptor, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL}, {contents, nullptr, DVASPECT_CONTENT, 0, TYMED_ISTREAM}, {effect, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL}};
        return SHCreateStdEnumFmtEtc(3, formats, out);
    }
    HRESULT STDMETHODCALLTYPE DAdvise(FORMATETC *, DWORD, IAdviseSink *, DWORD *) override { return OLE_E_ADVISENOTSUPPORTED; }
    HRESULT STDMETHODCALLTYPE DUnadvise(DWORD) override { return OLE_E_ADVISENOTSUPPORTED; }
    HRESULT STDMETHODCALLTYPE EnumDAdvise(IEnumSTATDATA **) override { return OLE_E_ADVISENOTSUPPORTED; }
    HRESULT STDMETHODCALLTYPE SetAsyncMode(BOOL value) override { asyncMode = value != FALSE; return S_OK; }
    HRESULT STDMETHODCALLTYPE GetAsyncMode(BOOL *out) override { if (!out) return E_POINTER; *out = asyncMode; return S_OK; }
    HRESULT STDMETHODCALLTYPE StartOperation(IBindCtx *) override { inOperation = true; return S_OK; }
    HRESULT STDMETHODCALLTYPE InOperation(BOOL *out) override { if (!out) return E_POINTER; *out = inOperation; return S_OK; }
    HRESULT STDMETHODCALLTYPE EndOperation(HRESULT, IBindCtx *, DWORD) override { inOperation = false; return S_OK; }
};

class Source final : public IDropSource {
    std::atomic<ULONG> refs{1};
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id, void **out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (id != IID_IUnknown && id != IID_IDropSource) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return ++refs; }
    ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs; if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE QueryContinueDrag(BOOL escape, DWORD keys) override {
        return escape ? DRAGDROP_S_CANCEL : ((keys & MK_LBUTTON) ? S_OK : DRAGDROP_S_DROP);
    }
    HRESULT STDMETHODCALLTYPE GiveFeedback(DWORD) override { return DRAGDROP_S_USEDEFAULTCURSORS; }
};
int immich_mouse_down(void) { return (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0; }
int immich_start_drag(void *, const char *name, const unsigned char *, size_t,
    const unsigned char *rgba, uint32_t width, uint32_t height, void *ctx, ImmichCallbacks cb) {
    if (!immich_mouse_down()) { cb.release(ctx); return 0; }
    HRESULT init = OleInitialize(nullptr);
    if (FAILED(init)) { cb.release(ctx); return 0; }
    {
        ComPtr<IDataObject> data; data.Attach(new FileData(ctx, cb, wide(name)));
        ComPtr<IDropSource> source; source.Attach(new Source());
        ComPtr<IDragSourceHelper> helper;
        HBITMAP bitmap = nullptr;
        if (SUCCEEDED(CoCreateInstance(CLSID_DragDropHelper, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&helper)))) {
            BITMAPINFO info{}; info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER); info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -static_cast<LONG>(height); info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32;
            void *pixels = nullptr; bitmap = CreateDIBSection(nullptr, &info, DIB_RGB_COLORS, &pixels, nullptr, 0);
            if (bitmap) {
                auto *out = static_cast<unsigned char *>(pixels);
                for (size_t i = 0; i < size_t(width) * height; ++i) {
                    unsigned a = rgba[i * 4 + 3];
                    out[i * 4] = rgba[i * 4 + 2] * a / 255; out[i * 4 + 1] = rgba[i * 4 + 1] * a / 255;
                    out[i * 4 + 2] = rgba[i * 4] * a / 255; out[i * 4 + 3] = a;
                }
                SHDRAGIMAGE image{{static_cast<LONG>(width), static_cast<LONG>(height)}, {static_cast<LONG>(width / 2), static_cast<LONG>(height / 2)}, bitmap, CLR_NONE};
                helper->InitializeFromBitmap(&image, data.Get());
            }
        }
        DWORD effect = 0; HRESULT result = DoDragDrop(data.Get(), source.Get(), DROPEFFECT_COPY, &effect);
        cb.ended(ctx, result == DRAGDROP_S_DROP);
        if (bitmap) DeleteObject(bitmap);
    }
    OleUninitialize(); return 1;
}
