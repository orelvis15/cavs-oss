package cavs

// ProgressEvent is one event emitted during a long-running operation.
type ProgressEvent struct {
	Type         string   `json:"type"`
	Operation    string   `json:"operation"`
	Phase        string   `json:"phase,omitempty"`
	CurrentBytes uint64   `json:"currentBytes,omitempty"`
	TotalBytes   uint64   `json:"totalBytes,omitempty"`
	Percentage   *float64 `json:"percentage,omitempty"`
	Message      string   `json:"message,omitempty"`
}

// RoutePolicy selects how routes are ranked when a policy applies.
type RoutePolicy string

const (
	PolicyBalanced    RoutePolicy = "balanced"
	PolicyNetworkMin  RoutePolicy = "networkMin"
	PolicyHDDFriendly RoutePolicy = "hddFriendly"
)

// AllRoutes is the sentinel meaning "model every route".
var AllRoutes []string

// ---- Analyze ----

type AnalyzeRequest struct {
	OldPath       string `json:"oldPath"`
	NewPath       string `json:"newPath"`
	EngineHint    string `json:"engineHint,omitempty"`
	MaxWorstFiles int    `json:"maxWorstFiles,omitempty"`
}

type WorstFile struct {
	Path                   string  `json:"path"`
	Status                 string  `json:"status"`
	IsPack                 bool    `json:"isPack"`
	OldSizeBytes           uint64  `json:"oldSizeBytes"`
	NewSizeBytes           uint64  `json:"newSizeBytes"`
	EstimatedDownloadBytes uint64  `json:"estimatedDownloadBytes"`
	ReuseRatio             float64 `json:"reuseRatio"`
	EntropyBits            float64 `json:"entropyBits"`
}

type Recommendation struct {
	Severity             string `json:"severity"`
	Kind                 string `json:"kind"`
	Title                string `json:"title"`
	File                 string `json:"file,omitempty"`
	EstimatedWastedBytes uint64 `json:"estimatedWastedBytes"`
	Why                  string `json:"why"`
	Fix                  string `json:"fix"`
	ExpectedImprovement  string `json:"expectedImprovement"`
}

type AnalyzeSummary struct {
	OldSizeBytes            uint64      `json:"oldSizeBytes"`
	NewSizeBytes            uint64      `json:"newSizeBytes"`
	EstimatedUpdateBytes    uint64      `json:"estimatedUpdateBytes"`
	EstimatedSteamPipeBytes uint64      `json:"estimatedSteamPipeBytes"`
	CavsReuseRatio          float64     `json:"cavsReuseRatio"`
	SteamPipeReuseRatio     float64     `json:"steamPipeReuseRatio"`
	FilesUnchanged          int         `json:"filesUnchanged"`
	FilesModified           int         `json:"filesModified"`
	FilesAdded              int         `json:"filesAdded"`
	FilesDeleted            int         `json:"filesDeleted"`
	WorstFiles              []WorstFile `json:"worstFiles"`
}

type AnalyzeReport struct {
	Summary         AnalyzeSummary   `json:"summary"`
	Engine          string           `json:"engine"`
	Warnings        []string         `json:"warnings"`
	Recommendations []Recommendation `json:"recommendations"`
	Note            string           `json:"note"`
}

// ---- Pack ----

type PackDirectoryRequest struct {
	InputDir    string   `json:"inputDir"`
	OutputCavs  string   `json:"outputCavs"`
	Profile     string   `json:"profile,omitempty"`
	Compression string   `json:"compression,omitempty"`
	SignKeyPath string   `json:"signKeyPath,omitempty"`
	Ignore      []string `json:"ignore,omitempty"`
}

type PackResult struct {
	OutputCavs      string `json:"outputCavs"`
	TotalSizeBytes  uint64 `json:"totalSizeBytes"`
	ChunkCount      uint64 `json:"chunkCount"`
	LogicalChunks   uint64 `json:"logicalChunks"`
	LogicalRawBytes uint64 `json:"logicalRawBytes"`
	StoredBytes     uint64 `json:"storedBytes"`
	MerkleRoot      string `json:"merkleRoot"`
	FilesPacked     uint64 `json:"filesPacked"`
	EntriesIgnored  uint64 `json:"entriesIgnored"`
	Signed          bool   `json:"signed"`
	Profile         string `json:"profile"`
	ElapsedMs       uint64 `json:"elapsedMs"`
}

// ---- Preview ----

type PreviewRequest struct {
	OldPath    string      `json:"oldPath"`
	NewPath    string      `json:"newPath"`
	EngineHint string      `json:"engineHint,omitempty"`
	Routes     []string    `json:"routes,omitempty"`
	Policy     RoutePolicy `json:"policy,omitempty"`
}

type Route struct {
	Name         string  `json:"name"`
	NetworkBytes uint64  `json:"networkBytes"`
	DiffMs       *uint64 `json:"diffMs,omitempty"`
	ApplyMs      *uint64 `json:"applyMs,omitempty"`
	Available    bool    `json:"available"`
}

type PreviewReport struct {
	RecommendedRoute string  `json:"recommendedRoute"`
	OldSizeBytes     uint64  `json:"oldSizeBytes"`
	NewSizeBytes     uint64  `json:"newSizeBytes"`
	Routes           []Route `json:"routes"`
	Explanation      string  `json:"explanation"`
}

// ---- Plan ----

type CreatePlanRequest struct {
	OldPath      string `json:"oldPath,omitempty"`
	OldSignature string `json:"oldSignature,omitempty"`
	NewPath      string `json:"newPath"`
	OutputPlan   string `json:"outputPlan"`
	PlanKind     string `json:"planKind,omitempty"`
	BlockKiB     uint32 `json:"blockKib,omitempty"`
	ZstdLevel    int    `json:"zstdLevel,omitempty"`
}

type PlanResult struct {
	PlanPath              string `json:"planPath"`
	PlanBytes             uint64 `json:"planBytes"`
	PlanKind              string `json:"planKind"`
	Mode                  string `json:"mode"`
	OperationCount        uint64 `json:"operationCount"`
	CopyOps               uint64 `json:"copyOps"`
	InlineOps             uint64 `json:"inlineOps"`
	ReusedBytes           uint64 `json:"reusedBytes"`
	InlineBytes           uint64 `json:"inlineBytes"`
	EstimatedNetworkBytes uint64 `json:"estimatedNetworkBytes"`
	ExpectedOutputSize    uint64 `json:"expectedOutputSize"`
	Files                 uint64 `json:"files"`
	UnchangedFiles        uint64 `json:"unchangedFiles"`
	Deleted               uint64 `json:"deleted"`
	ElapsedMs             uint64 `json:"elapsedMs"`
}

// ---- Apply ----

type ApplyPlanRequest struct {
	OldPath       string `json:"oldPath"`
	PlanPath      string `json:"planPath"`
	OutputPath    string `json:"outputPath"`
	CheckOld      bool   `json:"checkOld,omitempty"`
	DeleteRemoved bool   `json:"deleteRemoved,omitempty"`
}

type ApplyResult struct {
	OutputPath      string `json:"outputPath"`
	Verified        bool   `json:"verified"`
	Mode            string `json:"mode"`
	FilesTotal      uint64 `json:"filesTotal"`
	FilesWritten    uint64 `json:"filesWritten"`
	FilesNoop       uint64 `json:"filesNoop"`
	DirsCreated     uint64 `json:"dirsCreated"`
	SymlinksCreated uint64 `json:"symlinksCreated"`
	Deleted         uint64 `json:"deleted"`
	BytesWritten    uint64 `json:"bytesWritten"`
	BytesFromOld    uint64 `json:"bytesFromOld"`
	BytesFromBlob   uint64 `json:"bytesFromBlob"`
	ElapsedMs       uint64 `json:"elapsedMs"`
}

// ---- Verify ----

type VerifyRequest struct {
	Target     string `json:"target"`
	Signature  string `json:"signature,omitempty"`
	Manifest   string `json:"manifest,omitempty"`
	AllowExtra bool   `json:"allowExtra,omitempty"`
}

type Mismatches struct {
	Modified []string `json:"modified"`
	Missing  []string `json:"missing"`
	Extra    []string `json:"extra"`
}

type VerifyResult struct {
	Verified     bool       `json:"verified"`
	FilesChecked uint64     `json:"filesChecked"`
	BytesChecked uint64     `json:"bytesChecked"`
	Mismatches   Mismatches `json:"mismatches"`
	ElapsedMs    uint64     `json:"elapsedMs"`
}

// ---- Benchmark ----

type BenchmarkRequest struct {
	OldPath      string `json:"oldPath"`
	NewPath      string `json:"newPath"`
	EngineHint   string `json:"engineHint,omitempty"`
	MeasureApply bool   `json:"measureApply"`
}

type BenchmarkReport struct {
	OldSizeBytes     uint64  `json:"oldSizeBytes"`
	NewSizeBytes     uint64  `json:"newSizeBytes"`
	RecommendedRoute string  `json:"recommendedRoute"`
	Routes           []Route `json:"routes"`
	ReuseRatio       float64 `json:"reuseRatio"`
}

// ---- Savings ----

type SavingsRequest struct {
	PricePerGB               float64 `json:"pricePerGb"`
	MonthlyDownloads         float64 `json:"monthlyDownloads"`
	AverageFullDownloadBytes float64 `json:"averageFullDownloadBytes"`
	AverageCavsDownloadBytes float64 `json:"averageCavsDownloadBytes"`
}

type SavingsReport struct {
	FullDownloadMonthlyCost float64 `json:"fullDownloadMonthlyCost"`
	CavsMonthlyCost         float64 `json:"cavsMonthlyCost"`
	EstimatedMonthlySavings float64 `json:"estimatedMonthlySavings"`
	SavingsPercent          float64 `json:"savingsPercent"`
}
