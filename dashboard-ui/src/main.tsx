import React, { Suspense, lazy } from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, Routes, Route, Navigate, useLocation } from "react-router-dom";
import "./index.css";
import { AppLayout } from "@/components/app-layout";
import { LanguageProvider } from "@/lib/i18n";
import { ToastProvider } from "@/components/ui/toast";
import { ConfirmProvider } from "@/lib/confirm";

// code-splitting: nur login ist im initialen bundle, alle anderen seiten
// (inkl. recharts) werden erst beim ersten Besuch geladen
const Login = lazy(() => import("@/pages/Login"));
const Overview = lazy(() => import("@/pages/Overview"));
const Requests = lazy(() => import("@/pages/Requests"));
const RequestDetail = lazy(() => import("@/pages/RequestDetail"));
const Keys = lazy(() => import("@/pages/Keys"));
const Providers = lazy(() => import("@/pages/Providers"));
const Settings = lazy(() => import("@/pages/Settings"));

// Remount-trigger: seiten laden ihre daten in useEffect - ein key auf dem
// router-outlet erzwingt einen frischen load bei jedem seitenwechsel (z.b.
// nach einem provider-rename sofort aktuelle namen auf allen seiten).
function FreshRoutes() {
  const location = useLocation();
  return (
    <Routes location={location} key={location.pathname}>
      <Route
        path="/login"
        element={
          <Suspense fallback={null}>
            <Login />
          </Suspense>
        }
      />
      <Route element={<AppLayout />}>
        <Route
          path="/"
          element={
            <Suspense fallback={null}>
              <Overview />
            </Suspense>
          }
        />
        <Route
          path="/requests"
          element={
            <Suspense fallback={null}>
              <Requests />
            </Suspense>
          }
        />
        <Route
          path="/requests/:id"
          element={
            <Suspense fallback={null}>
              <RequestDetail />
            </Suspense>
          }
        />
        <Route
          path="/keys"
          element={
            <Suspense fallback={null}>
              <Keys />
            </Suspense>
          }
        />
        <Route
          path="/providers"
          element={
            <Suspense fallback={null}>
              <Providers />
            </Suspense>
          }
        />
        <Route
          path="/settings"
          element={
            <Suspense fallback={null}>
              <Settings />
            </Suspense>
          }
        />
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <LanguageProvider>
      <ToastProvider>
        <ConfirmProvider>
          <BrowserRouter>
            <FreshRoutes />
          </BrowserRouter>
        </ConfirmProvider>
      </ToastProvider>
    </LanguageProvider>
  </React.StrictMode>
);
