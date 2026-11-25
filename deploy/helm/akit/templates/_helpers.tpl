{{/* The release's full name: the release's, or <release>-akit. */}}
{{- define "akit.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "akit.selectorLabels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "akit.labels" -}}
{{ include "akit.selectorLabels" . }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "akit.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- .Values.serviceAccount.name | default (include "akit.fullname" .) -}}
{{- else -}}
{{- .Values.serviceAccount.name | default "default" -}}
{{- end -}}
{{- end -}}

{{/*
akit's config: .Values.config when set; otherwise a listener of akit's own
answering the kubelet's probes on /healthz, and the Kubernetes provider,
which adds the Gateways' listeners and routes, and the Ingresses'.
*/}}
{{- define "akit.config" -}}
{{- if .Values.config -}}
{{- toPrettyJson .Values.config -}}
{{- else -}}
{{- $k := dict "controller_name" .Values.controllerName -}}
{{- if .Values.ingress.enabled -}}
{{- $_ := set $k "ingress" (dict "http_port" (int .Values.ingress.httpPort) "https_port" (int .Values.ingress.httpsPort)) -}}
{{- end -}}
{{- $k = mergeOverwrite $k (deepCopy .Values.provider) -}}
{{- $health := dict "name" "health" "address" (printf ":%d" (int .Values.healthPort)) "protocols" (list "http/1.1") -}}
{{- $healthz := dict "name" "healthz" "listeners" (list "health") "match" (dict "path" (list "/healthz")) "handler" (dict "module" "respond" "params" (dict "body" "ok\n")) -}}
{{- toPrettyJson (dict "listeners" (list $health) "routes" (list $healthz) "providers" (list (dict "kubernetes" $k))) -}}
{{- end -}}
{{- end -}}
