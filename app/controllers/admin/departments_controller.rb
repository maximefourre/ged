class Admin::DepartmentsController < Admin::BaseController
  def index
    @departments = Department.with_deleted.includes(:parent).order(:name)
    @doc_counts = Document.group(:department_id).count
    @trashed_counts = Document.deleted.group(:department_id).count
  end

  def new
    @department = Department.new
  end

  def create
    @department = Department.new(params.require(:department).permit(:name, :parent_id))
    if @department.save
      redirect_to admin_departments_path, notice: "Department created."
    else
      flash.now[:alert] = @department.errors.full_messages.join(", ")
      render :new, status: :unprocessable_entity
    end
  end

  def edit
    @department = Department.find(params[:id])
  end

  # Rename only: re-parenting a department would silently move its whole
  # subtree across the permission boundary.
  def update
    @department = Department.find(params[:id])
    if @department.update(params.require(:department).permit(:name))
      redirect_to admin_departments_path, notice: "Department renamed."
    else
      flash.now[:alert] = @department.errors.full_messages.join(", ")
      render :edit, status: :unprocessable_entity
    end
  end

  def destroy
    department = Department.with_deleted.find(params[:id])
    if Department.with_deleted.where(parent_id: department.id).exists?
      redirect_to admin_departments_path,
                  alert: "Cannot delete #{department.name}: it has sub-departments. Delete or move them first."
    elsif Document.with_deleted.where(department_id: department.id).exists? ||
          department.upload_requests.exists?
      redirect_to confirm_deletion_admin_department_path(department)
    else
      department.destroy!
      redirect_to admin_departments_path, notice: "Department deleted."
    end
  rescue ActiveRecord::InvalidForeignKey
    redirect_to admin_departments_path,
                alert: "Department is still referenced by other records and cannot be deleted."
  end

  def confirm_deletion
    @department = Department.with_deleted.find(params[:id])
    @live_docs_count = Document.where(department_id: @department.id).count
    @trashed_docs_count = Document.deleted.where(department_id: @department.id).count
    @pending_requests_count = @department.upload_requests.count
    @target_departments = Department.where.not(id: @department.id).order(:name)
    @default_target = @department.parent if @department.parent && @target_departments.exists?(@department.parent_id)
  end

  def move_documents
    source = Department.with_deleted.find(params[:id])
    if Department.with_deleted.where(parent_id: source.id).exists?
      return redirect_to admin_departments_path,
                         alert: "Cannot move: #{source.name} has sub-departments."
    end
    target = Department.find(params[:target_department_id])
    if target.id == source.id
      return redirect_to confirm_deletion_admin_department_path(source),
                         alert: "Choose a different destination department."
    end
    now = Time.current
    ActiveRecord::Base.transaction do
      Document.with_deleted.where(department_id: source.id)
              .update_all(department_id: target.id, updated_at: now)
      UploadRequest.where(department_id: source.id)
                   .update_all(department_id: target.id, updated_at: now)
      source.destroy!
    end
    redirect_to admin_departments_path,
                notice: "Moved documents to #{target.name} and deleted #{source.name}."
  end

  def trash
    department = Department.find(params[:id])
    department.soft_delete!
    redirect_to admin_departments_path,
                notice: "#{department.name} moved to trash. It will be removed automatically once its documents are purged."
  end

  def restore
    department = Department.deleted.find(params[:id])
    department.restore!
    redirect_to admin_departments_path, notice: "#{department.name} restored."
  end
end
