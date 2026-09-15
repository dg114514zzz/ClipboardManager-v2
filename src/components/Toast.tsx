export default function Toast({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <div
      className="fixed bottom-12 left-1/2 -translate-x-1/2 bg-bg-active text-text-primary px-4 py-1.5 rounded-md text-sm shadow-lg z-50"
      style={{
        position: "fixed",
        bottom: 48,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 50,
        background: "rgba(30, 30, 30, 0.92)",
        color: "#e8e8e8",
        padding: "6px 14px",
        borderRadius: 6,
        boxShadow: "0 4px 14px rgba(0,0,0,0.45)",
        whiteSpace: "nowrap",
        fontSize: 14,
      }}
    >
      {message}
    </div>
  );
}
